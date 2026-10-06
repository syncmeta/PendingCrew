# crewd M2b claude 会话驱动 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 新建 crate `crewd-exec`：在 PTY 里起 claude（找可执行文件、白名单环境、argv 与 0600 临时文件），用 M2a 的 `crewd-term` 读屏驱动开场 / 运行中投递，认健康（额度、限流菜单、认证、拉起失败），停止与孤儿回收，会话号落账与 `--resume`（被拒自动换新号重起一次）；对外只有一个窄接口 `SessionHandle` + 事件流。配一个假 claude 测试器材做端到端，和一条默认不跑的真机冒烟；`crewd claude` 子命令让它能从命令行驱动。

**Architecture:** 三层。**纯计算层**：`delivery`（投递状态机）、`health`（健康判定）、`session::Core`（整个会话的 sans-IO 判断：喂字节 / 定时拍 / 命令 / 退出 → 吐出「往 PTY 写什么」和「发生了什么」，时间由调用方传入）。**进程层**：`env`（登录 shell PATH、找 claude、白名单环境）、`launch`（argv 与临时文件、hooks/MCP 命令行）、`pty`（手写 rustix-openpty + tokio）、`registry`（孤儿登记与回收）、`resume`（会话号账本 trait）。**驱动层**：`driver`（一个 tokio 任务拥有 PTY 主端、子进程和 `Core`；命令从 `SessionHandle` 进，`SessionEvent` 从 mpsc 出；写 PTY 走单独的写任务）。`crewd-exec` 只依赖 `crewd-term`；账本的 SQLite 实现（`StoreLedger`）在 `crewd-server`，`crewd claude` 把它们接起来。spec §13 Q12 要的窄接口就是 `SessionHandle`：以后执行器拆成独立进程，只把命令与事件换成线协议。

**Tech Stack:** Rust 1.94（edition 2024）、tokio 1.53（rt / macros / time / sync / process / net）、rustix 1 + rustix-openpty 0.2（alacritty_terminal 已把它们带进 Cargo.lock，不新增第三方 crate）、libc、chrono（关掉默认特性）、serde / serde_json、uuid v4、crewd-term（M2a）。

**Spec:** `docs/superpowers/specs/2026-10-05-crewd-backend-and-work-tracking-design.md`（§3.3、§13 Q12、§14、§15、§16）；移植图 `docs/superpowers/specs/2026-10-05-crewd-m2-executor-porting-map.md`（§1、§2、§5、§6、§8、§9、§10）。Swift 对照：`Sources/Mac/LocalRunner/{AgentSessionCore,StartupPromptDelivery,SessionHealth,SessionLaunchProbe,SessionOrphanReaper,LocalSessionLaunch,LocalCodingAgentExecutable,LocalCodingAgentSpec,SessionConfig,AgentSessionResume}.swift` 与 `Tests/PendingCrewTests/` 下的同名测试。

> 写计划时已把本计划的全部代码在 main@a439b96（`crewd/` 与 aece5fc 相同）的副本里按任务顺序重放：每个任务结束 `check.sh` 全绿。crewd-exec 单测数依次 8 / 16 / 22 / 36 / 49 / 72 / 78（Task 1–7），Task 9 后 81；crewd-store 单测 9 → 10（Task 8）→ 11（Task 9）；crewd-server 单测 2 → 4（Task 9）→ 5（Task 12）；集成测试 `driver` 11、`fake_claude` 10 + `live_claude` 1 ignored、`claude_cli` 1。各「确认失败」步骤都真红过，各变异步骤逐条核过会红（红哪几条写在步骤里）。执行时数字对不上就停下查，别改期望凑数。**没做过的两件事**：没在 Linux 上编译（Task 12 Step 8 给了检查办法）；没跑过真 claude（Task 11 Step 8，要人点头才跑）。

## Global Constraints

- 所有命令在 `crewd/` 目录下跑；Rust 工具链在 `/opt/homebrew/opt/rustup/bin`（找不到 `cargo` 先 `export PATH=/opt/homebrew/opt/rustup/bin:$PATH`）。
- 依赖一律用 `cargo add --offline` 加（`--offline` = 只用本机已缓存的版本，也就是 Cargo.lock 里已有的那些），**不要手写版本号**。`rustix@1`、`rustix-openpty@0.2` 与 alacritty_terminal 0.26 用的是同一线；整个 M2b 不往 Cargo.lock 里加任何第三方 crate，只多出 `crewd-exec` 自己那一条。`Cargo.lock` 要提交。
- 依赖方向：`crewd-exec` → `crewd-term`；`crewd-server` → `crewd-exec`；`crewd`（二进制）→ 两者。**`crewd-exec` 不依赖 store / server / proto**：账本是 trait（Q12：执行器只经窄接口与其余部分交互）。
- 窄接口 = `driver::{start_claude, ClaudeStart, DriverConfig, SessionHandle, SessionStatus, StartError}` + `session::SessionEvent` 及其载荷类型。外部（server / M3）不碰 `pty`、`session::Core`。公开类型里不出现 alacritty 类型（M2a 约束延续）。
- 纯计算层（`delivery`；`health` 里除 `AuthProbe::read` 读文件外；`session::Core`）不起进程、不碰 tokio、不调 `Instant::now()`——时间一律由调用方传入（同 M2a 裁定 3，不引入 `Clock` trait）。驱动层用真时钟；它的测试用真进程、真时间，靠缩短 `DriverConfig` 里的窗口提速。
- **首条 prompt 绝不进 argv**（`ps` 对本机所有进程可见，2026-08-28 出过事故）；`--permission-mode auto`，绝不 bypass。
- 子进程环境是白名单拼出来的整份（`env_clear`），不继承 crewd 的环境。
- 启动临时文件放 `<数据目录>/run/claude-<会话号>/`（目录 0700、文件 0600），会话结束删掉；不放 `$TMPDIR` / `/tmp`。
- 测试**只用合成数据** + M2a 已入库的 4 个夹具（`crates/term/tests/fixtures/*.bin`，缺了必须失败，绝不静默跳过）。不读真实 `~/.claude`、不碰真实数据目录；会起登录 shell 的测试把 `SHELL` 设成 `/usr/bin/false`，不依赖本机 rc 文件。
- 真 claude 只出现在 `tests/live_claude.rs`：`#[ignore]` + 环境变量 `CREWD_LIVE_CLAUDE=1` 两道闸；`check.sh` 不带 `--ignored`，**永远不跑它**。
- 测试里起的、可能被 killpg 的进程一律自成进程组：判定写错时 killpg 不能落到跑测试的那一组上（写计划时一次变异就这样把 cargo test 连同终端一起带走了）。
- 本机 shell 是 zsh，不做词分割：手动跑变异步骤时照抄给出的命令，别把 `--test driver` 这类参数塞进一个变量再展开（会变成一个参数，cargo 报错、grep 什么也抓不到，看起来像「没红」）。
- 每个任务结束跑 `./scripts/check.sh`（fmt + clippy -D warnings + test）全绿再提交。提交只 `git add` 该任务列出的文件（路径相对 `crewd/`），提交信息末尾加 `Co-Authored-By: Claude <noreply@anthropic.com>`（不带模型名）。
- 在分支 `crewd-m2b` 上做（执行时用 worktree）。

## 范围

做：找 claude 可执行文件（登录 shell PATH 带哨兵 + 5s 超时、childProcessPath 合并、环境白名单与禁用前缀、TERM）；claude argv（`--resume` / `--session-id`、`--permission-mode auto`、`--append-system-prompt-file`、`--settings`、`--mcp-config`、`--model`、`--effort`；fast 模式）与 0600 临时文件、指向可配置 helper 的 hooks / MCP 命令行（hooks 可关）；PTY（setsid、TIOCSCTTY、SIGPIPE 复位、进程组、非阻塞主端、tokio 异步读写）；会话驱动（窄接口、事件、节拍、注入时间）；投递状态机（等就绪 → 写正文 → 回读落地 → 空格 / 退格 / 回车三拍提交 → 回读；Ctrl-U 重投；3 次；对话框 3s；90s 没就绪；一次一笔、排队）；健康（额度短语、限流菜单自动应答 + 30s 冷却、额度恢复 6s、认证探测 2s、拉起自检 25s）；续跑被拒 5s 内识别 + 换新号重起一次；停止（SIGTERM → 2s → killpg SIGKILL）、退出状态解码、孤儿登记与回收（启动时刻核对，macOS / Linux 各一份）；会话号账本落在 M1 的 sessions 表；spec §16 要求的执行器类型字段；CRLF 字节尾窗的裁定；假 claude 器材 + 端到端；真机冒烟（opt-in）；`crewd claude` 子命令；serve 启动时回收孤儿。

不做：codex（M2c）；attach 协议与快照帧（M2d——`SessionHandle::snapshot` 只把 M2a 的 `Snapshot` 交出去）；`crewd mcp` / `crewd hook` 子命令本体与世界观渲染（M3——M2b 只把 helper 路径当参数，命令行形状收在 `HelperCommands` 一处）；中途切模型 `/model`（`SessionProfileSwitch`，唯一调用方是 M3 的 `set_session_profile`）与启动参数回显扫描（`SessionLaunchParameterScanner`，同样等 M3）；awaitingReply（依赖 M3 的 Stop hook）；机长 checkpoint、唤醒准入、群消息渲染；serve 里的会话管理（M3：serve 拉起会话、登记表交给会话管理）。

## 写计划时定下的裁定（执行时照做，有异议先回报）

1. **crate 叫 `crates/exec`**（包 `crewd-exec`，库 `crewd_exec`），不叫 session：「session」在仓库里已经有三个意思（crewd 的 session 行、claude 的会话号、Swift 的 `SessionBackend`）；spec 管它叫「执行器」；M2c 的 codex、M2e 的 ACP 各自加一个驱动进同一个 crate，共用 `env` / `pty` / `registry` / `resume` 和窄接口。crate 里那个 sans-IO 核心模块叫 `session`（`session::Core`），它确实是「一个会话」。
2. **PTY 手写**（rustix-openpty + `tokio::process` + `AsyncFd`），不用 `alacritty_terminal::tty`、不用 portable-pty：
   - `alacritty_terminal::tty`（0.26 源码核过）：子进程**继承父进程全部环境**再加 `ALACRITTY_WINDOW_ID` / `WINDOWID`（与白名单冲突，没有 `env_clear` 的口子）；cwd 不存在时**静默忽略**（`chdir` 失败不报）；`Drop` 里阻塞 `child.wait()`（会卡住 tokio 工作线程）；绑死它自己的 `polling` 事件循环与 signal-hook SIGCHLD。
   - portable-pty：读写是阻塞的 `Read` / `Write`（每会话两条线程），子进程等待是阻塞 / 轮询，Windows ConPTY 用不上；还会引入一批新 crate（nix、filedescriptor 等；写计划时没逐个核）。
   - 手写约 150 行：主端挂 `AsyncFd`（macOS kqueue / Linux epoll；alacritty 自己也是用 kqueue 轮询 PTY 主端，本计划的测试在 macOS 上实跑过）；`tokio::process::Child` 给异步退出状态——Swift #541 那类「退出回调丢了，状态停在 running 装空闲」在这里结构上不会发生；`env_clear`；cwd 不存在 = 启动失败；SIGPIPE 由 std 在 exec 前复位（有测试钉住）。rustix / rustix-openpty 已经在 Cargo.lock 里。spec §3.3 写的是 portable-pty，Task 12 改 §3.3 并在 §17 记下这条。
3. **CRLF 字节尾窗：不改尾窗，执行器只用屏幕判待决策**（`Core` 不喂 `PendingDecisionTracker` 的字节尾窗）。理由：生产 PTY 行尾是 CRLF，尾窗里多行菜单永远认不出（spec §15）；就算折掉 CRLF，尾窗也不知道屏幕已经翻页，答完的菜单会在 4K 窗里挂到被新输出挤出去为止，把「已清除」拖住——这比认不出更糟（谎报「在等人」）。额度 / 限流 / 续跑被拒这些单行短语照旧用尾窗。有测试钉住（`answered_menu_clears_with_the_screen_not_the_byte_tail`）。
4. **输入行要连续在屏 300ms 才算就绪**（`Timing::ready_settle`，Swift 没有）。证据：把 `tui-claude-trust.bin` 按 64 字节切着喂，第 1024 字节处信任框画到一半，`❯  No, exit` 单独在屏、页脚还没出来，`input_row` 返回 `Some("No, exit")`、`blocking_dialog` 是 `None`——不等稳就会把开场任务当按键敲进信任框（回车选中「退出」）。`tui-claude.bin` 在 2304 字节处也有一帧同类的过渡。
5. **`BlockedByDialog` / `NotReady` 只报不终止**：答完对话框、输入框终于画出来，照样补投，补投成功清掉那条 health。这是 Swift 状态机自己测试里写明的意图（「答完之后还要把开场任务补投出去」）；Swift 驱动层 `AgentSessionCore.stepPromptDelivery` 却把这两种当终局调了 `finishDelivery`、把开场任务丢了——两边矛盾，按状态机的意图移植。
6. **提交三拍发完之前不回读**：Swift 发完空格立刻允许下一拍判「输入行变了 = 提交成功」。粘贴占位符会把那个空格算成多一行（`+2 lines` → `+3 lines`），那一帧会被误判成功。`Core` 在排好的按键（退格、回车）没发完之前不推进投递机。
7. **正文去毒**：写进 PTY 前 CR / CRLF 一律变 LF（单独一个 CR 就是「提交」），其余控制字符（ESC 会打断回合、Ctrl-C 会中止、NUL…）换成 Unicode 控制图形（U+2400 起），换行与制表符原样。Swift 原样写。
8. **TERM 固定 `xterm-256color`**，不透传 crewd 所在终端的 TERM（子进程的终端是我们的 `Screen`，不是开发者的 ghostty / iTerm）；白名单透传 HOME / SHELL / LANG / USER / LOGNAME / TMPDIR 与 `LC_*`；额外变量拒收 `PENDINGBOT_` / `PENDINGCREW_` / `SUPABASE_` / `CREWD_` 前缀（多了 `CREWD_`）。codex 的 `OPENAI_API_KEY` 剥离归 M2c。
9. **续跑**：账本记过就 `--resume`（不预判它在不在），没记过生成小写 UUID v4、**起进程前先落账**（落账失败 = 不起进程）。启动后 5s 内屏幕**或**字节尾窗出现 `No conversation found with session ID` + 本 id → 发 `ResumeRefused`，杀掉、换新号（先落账）重起一次，没投出去的正文搬到新进程，事件流与句柄不变。尾窗那一路是为了窄屏：attach 的客户端把终端改窄后那行原话在屏上折成两行。
10. **孤儿身份**：macOS 用 `proc_pidinfo(PROC_PIDTBSDINFO)` 的启动时刻（秒 + 微秒，与 sysctl `KERN_PROC` 的 `p_starttime` 同一个内核字段——libc 0.2 没给 macOS 导出 `kinfo_proc`），Linux 读 `/proc/<pid>/stat` 第 22 字段（时钟滴答）与第 5 字段（pgrp）；都折成一个 `u64` 只在本机比较。读不到身份（权限）的那条**不动手、留在表里**。登记表是 `<数据目录>/exec-registry.json`（0600、整份原子替换）；坏了就报错，不能当空表。
11. **账本是 trait**（`resume::AgentIdLedger`）：`crewd-exec` 不依赖存储。SQLite 实现 `crewd_server::ledger::StoreLedger` 每次读写各开一条连接（一次启动只读写一两回，不跟 RPC 抢 `AppState` 的锁），驱动层在 `spawn_blocking` 里调。
12. **执行器类型字段（spec §16「M2b 起就按此建」）**：schema v2 把 `sessions.agent_kind` 改名 `executor`，`claude_code` → `claude`，`codex` 不变，旧数据的 `terminal` / `unknown` 原样留着；proto `Session.agent_kind` 同步改名 `executor`（线上 JSON `executor`），契约文件重新生成；旧 JSON 导入写新词表。`crews.captain_agent_kind` 不在 §16 的范围，不动（见「遗留」）。
13. **写 PTY 走单独的写任务**：子进程不读输入时写会阻塞；写放在驱动循环里就会卡住读，双方互等。写任务按顺序写，驱动循环永远不等写完。
14. **helper 命令行**：MCP = `<helper> mcp --data-dir D --crew C --session S --agent claude [--captain] [--structured-post] [--label L]`（exec 形式、不加引号）；hook = `<helper> hook <post-tool-use|pre-tool-use|stop> --data-dir D --crew C --session S …`（每个词单引号的 shell 串）。旧版的 `--dir <白板目录>` 换成 `--data-dir`（helper 经 crewd 的 socket 说话）。子命令名由 M3 定稿，改只改 `HelperCommands`。`CommsSpec::hooks = false` 时不注册任何 hook（M3 之前 helper 还没有 `hook` 子命令）。`crewd claude` 不挂 comms。
15. **所有句柄都 drop 了 = 停掉会话**（没人能再控制它，不能让它变成孤儿）。
16. **节拍**：每块 PTY 输出一拍、0.6s 定时一拍、`Core::next_wakeup()` 到点一拍（提交三拍之间的 100ms、限流应答的 250ms 要比 0.6s 准）。
17. **winsize 像素**报 `cols×16 × lines×16`，与 `Screen` 对 `CSI 14 t` 的回答一致（Swift 报的是 16×16 常量）。
18. 额度扫描器 `rearm` 时连尾窗一起清掉（Swift 不清，重新武装后旧短语会被下一块输出立刻再命中）。

## Review Focus

1. 正文里带控制字符（白板里贴进来的终端输出：ESC、Ctrl-C、NUL）→ 绝不能以按键的形式到达 claude（ESC 打断回合、Ctrl-C 中止）。（Task 6 `control_characters_in_text_are_made_inert`）
2. 子进程不读输入（卡死 / 正忙），我们却要写一大段 → 会话不能被卡死：状态照样查得到、停得掉。（Task 11 `a_child_that_never_reads_cannot_wedge_the_session`）
3. attach 的客户端把终端改窄（M2d 会先 resize 权威终端），续跑被拒那行原话在屏上折成两行 → 照样认出、照样换新号重起。（Task 6 `refusal_is_recognised_even_when_wrapped_on_a_narrow_screen`）
4. 会话号账本写不进去（session 行被删了 / 库坏了）→ 绝不起进程（起了也接不回来，下次重启又是新对话），报 `StartError::Ledger`。（Task 10 `ledger_failure_means_no_process`）
5. 几十 KB 的开场任务（首轮白板 + 任务说明）→ 只提交一次、一字不差，不因为验证窗口超时重投出两份。（Task 11 `huge_brief_is_submitted_exactly_once`）

## 文件结构

```
crewd/crates/exec/
  Cargo.toml                   cargo new + cargo add 生成                                     (T1–T7)
  src/lib.rs                   模块声明（每个任务追加一行）
  src/env.rs                   登录 shell PATH、childProcessPath、找可执行文件、环境白名单        (T1)
  src/launch.rs                ClaudeArgs::argv、prepare_claude（0600 文件）、HelperCommands      (T2)
  src/pty.rs                   spawn_pty（setsid / TIOCSCTTY / env_clear）、PtyMaster 异步读写     (T3)
  src/delivery.rs              投递状态机 Delivery / Timing / Step                               (T4)
  src/health.rs                Health、额度 / 限流扫描、额度恢复、认证探测、拉起自检判定             (T5)
  src/session.rs               Core（sans-IO 会话核心）、SessionEvent、sanitize_text、resume_rejection (T6)
  src/registry.rs              ProcessIdentity / probe（macOS / Linux）、判定、ProcessRegistry、reap  (T7)
  src/resume.rs                AgentIdLedger、MemoryLedger、decide_resume                        (T9)
  src/driver.rs                start_claude、SessionHandle、驱动任务、写任务、续跑被拒重起          (T10)
  src/bin/fake_claude.rs       测试器材：假 claude                                             (T11)
  tests/driver.rs              驱动层：shell 脚本顶替 claude                                     (T10)
  tests/fake_claude.rs         端到端：投递、粘贴陷阱、信任框回放、限流、额度、Review Focus 2/5     (T11)
  tests/live_claude.rs         真 claude 冒烟（#[ignore] + CREWD_LIVE_CLAUDE=1）                 (T11)
crewd/crates/store/src/schema.rs     v2：agent_kind → executor                               (T8)
crewd/crates/store/src/sessions.rs   列改名（T8）；agent_session_id / set_agent_session_id（T9）
crewd/crates/store/src/lib.rs        测试跟着改名（T8）；账本测试（T9）
crewd/crates/proto/src/model.rs      Session.agent_kind → executor                           (T8)
crewd/crates/legacy/src/{sessions,lib}.rs  导入写新词表                                       (T8)
crewd/schema/crewd-api.schema.json   重新生成                                                (T8)
crewd/crates/server/src/ledger.rs    StoreLedger                                             (T9)
crewd/crates/server/src/lib.rs       pub mod ledger（T9）；reap_orphans + serve 启动回收（T12）
crewd/crates/crewd/src/claude_cmd.rs `crewd claude` 子命令                                    (T12)
crewd/crates/crewd/src/main.rs       接上子命令                                              (T12)
crewd/crates/crewd/tests/claude_cli.rs  库里落账 → 第二次 --resume                             (T12)
crewd/README.md、docs/superpowers/specs/2026-10-05-crewd-backend-and-work-tracking-design.md（§3.3 一行、§17）(T12)
```

## 窄接口一览（Task 10 交付；M3 的会话管理只用这些）

```rust
pub async fn start_claude(start: ClaudeStart, config: DriverConfig)
    -> Result<(SessionHandle, mpsc::UnboundedReceiver<SessionEvent>), StartError>;

impl SessionHandle {            // Clone；全部 drop = 停掉会话
    pub fn send(&self, text: impl Into<String>) -> Result<u64, Closed>;   // 投递编号，结果见 Delivery 事件
    pub fn raw(&self, bytes: impl Into<Vec<u8>>) -> Result<(), Closed>;   // 菜单按键 / Esc
    pub fn resize(&self, cols: u16, lines: u16) -> Result<(), Closed>;
    pub fn stop(&self) -> Result<(), Closed>;                             // SIGTERM → grace → killpg SIGKILL
    pub fn clear_quota_health(&self) -> Result<(), Closed>;
    pub async fn screen_text(&self, max_lines: usize) -> Result<String, Closed>;
    pub async fn snapshot(&self) -> Result<Snapshot, Closed>;
    pub async fn status(&self) -> Result<SessionStatus, Closed>;
}

pub enum SessionEvent {         // 第一条一定是 Spawned
    Spawned { pid, agent_session_id, resumed },
    Working(bool),              // false = idle 边沿
    Decision(DecisionEvent),    // Appeared(PendingDecision) / Cleared
    Delivery { id, subject, outcome },
    Health(Option<Health>),     // AuthRequired / UsageLimit / RateLimited / LaunchFailed / BriefUndelivered / PromptUndelivered
    ResumeRefused { id, said },
    Exited(ExitInfo),           // status, stopped_by_user, hit_limit；最后一条
}
```

---

### Task 1: crate 骨架 + 找可执行文件与子进程环境（`env`）

从 launchd / systemd 拉起的 crewd 只有最小 PATH，装在 `~/.local/bin`、npm、bun 下的 `claude` 找不到（Swift 版吃过这个亏）。起一次登录交互 shell 把 `$PATH` 抓回来（哨兵摘噪声、5s 超时会杀掉 shell），并上父进程 PATH 与系统目录，**同一条 PATH 既用来找可执行文件、也注入子进程**。子进程环境是白名单拼出来的整份（裁定 8）。

**Files:**
- Create: `crewd/crates/exec/Cargo.toml`（`cargo new` + `cargo add`）、`crewd/crates/exec/src/lib.rs`、`crewd/crates/exec/src/env.rs`
- Modify: `crewd/Cargo.lock`

**Interfaces:**
- Consumes: 无（tokio、rustix）。
- Produces（`crewd_exec::env`）:
  - 常量 `PATH_SENTINEL_OPEN` / `PATH_SENTINEL_CLOSE: &str`、`LOGIN_SHELL_TIMEOUT: Duration`（5s）、`SYSTEM_DIRS: [&str; 4]`、`PASSTHROUGH_KEYS: [&str; 6]`、`FORBIDDEN_PREFIXES: [&str; 4]`、`CHILD_TERM: &str`（`"xterm-256color"`）
  - `pub struct AgentPaths { pub child_path: String }`：`async fn discover() -> AgentPaths`、`fn resolve(&self, name: &str) -> Option<PathBuf>`
  - `async fn login_shell_path(shell: &Path, timeout: Duration) -> Option<String>`、`fn extract_sentinel_path(output: &str) -> Option<String>`
  - `fn compose_child_path(search_dirs: &[String], parent_path: Option<&str>) -> String`、`fn resolve_in_path(name: &str, path: &str) -> Option<PathBuf>`
  - `fn is_forbidden_key(key: &str) -> bool`、`fn build_env<'a>(parent: impl IntoIterator<Item = (&'a str, &'a str)>, child_path: &str, additional: &BTreeMap<String, String>) -> BTreeMap<String, String>`、`fn build_env_from_process(child_path: &str, additional: &BTreeMap<String, String>) -> BTreeMap<String, String>`

- [ ] **Step 1: 建 crate、加依赖**

```bash
cargo new --lib crates/exec --name crewd-exec --vcs none
cargo add --offline -p crewd-exec tokio --features rt,macros,time,sync,process,net
cargo add --offline -p crewd-exec rustix@1 --features termios,process,fs,stdio
cargo add --offline -p crewd-exec --dev tempfile
cat crates/exec/Cargo.toml
```

Expected（workspace 继承的字段 cargo 会自动写；若不是，手工改成这样）：

```toml
[package]
name = "crewd-exec"
edition.workspace = true
version.workspace = true
license.workspace = true

[dependencies]
rustix = { version = "1", features = ["termios", "process", "fs", "stdio"] }
tokio = { version = "1.53.2", features = ["rt", "macros", "time", "sync", "process", "net"] }

[dev-dependencies]
tempfile = "3.27.0"
```

- [ ] **Step 2: 写失败的测试**

`crates/exec/src/lib.rs`（整个文件，覆盖 `cargo new` 生成的内容）：

```rust
//! crewd 的执行器：起 agent 进程（PTY）、读它的屏幕、投递正文、认健康状态、收尸与恢复。
//! 其余部分只经 `driver::SessionHandle` 这一个窄接口与它交互（spec §13 Q12）。
pub mod env;
```

`crates/exec/src/env.rs`：先只放测试模块（下一步把实现写在它**上面**）：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::time::Instant;

    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn sentinel_is_found_amid_rc_noise() {
        let out = format!("motd\n{PATH_SENTINEL_OPEN}/a:/b c{PATH_SENTINEL_CLOSE}\nbye");
        assert_eq!(extract_sentinel_path(&out).as_deref(), Some("/a:/b c"));
        assert_eq!(extract_sentinel_path("no sentinel"), None);
        assert_eq!(
            extract_sentinel_path(&format!("{PATH_SENTINEL_OPEN}{PATH_SENTINEL_CLOSE}")),
            None
        );
        assert_eq!(
            extract_sentinel_path(&format!("{PATH_SENTINEL_OPEN}/a")),
            None
        );
    }

    #[tokio::test]
    async fn login_shell_output_is_parsed() {
        let dir = tempfile::tempdir().unwrap();
        let shell = script(
            dir.path(),
            "noisy-shell",
            &format!(
                "echo welcome; printf '{PATH_SENTINEL_OPEN}/x/bin:/y/bin{PATH_SENTINEL_CLOSE}'"
            ),
        );
        let got = login_shell_path(&shell, Duration::from_secs(5)).await;
        assert_eq!(got.as_deref(), Some("/x/bin:/y/bin"));
    }

    #[tokio::test]
    async fn hanging_login_shell_times_out() {
        let dir = tempfile::tempdir().unwrap();
        let shell = script(dir.path(), "stuck-shell", "sleep 30");
        let started = Instant::now();
        let got = login_shell_path(&shell, Duration::from_millis(300)).await;
        assert_eq!(got, None);
        assert!(started.elapsed() < Duration::from_secs(5), "超时必须生效");
    }

    #[tokio::test]
    async fn failing_or_missing_shell_gives_none() {
        let dir = tempfile::tempdir().unwrap();
        let shell = script(
            dir.path(),
            "bad-shell",
            &format!("printf '{PATH_SENTINEL_OPEN}/x{PATH_SENTINEL_CLOSE}'; exit 3"),
        );
        assert_eq!(login_shell_path(&shell, Duration::from_secs(5)).await, None);
        let missing = dir.path().join("nope");
        assert_eq!(
            login_shell_path(&missing, Duration::from_secs(5)).await,
            None
        );
    }

    #[test]
    fn child_path_prefers_login_dirs_then_parent_then_system() {
        let search = vec![
            " /home/u/.local/bin ".to_string(),
            "/usr/bin".to_string(),
            String::new(),
        ];
        let path = compose_child_path(&search, Some("/opt/x:/home/u/.local/bin::/bin"));
        assert_eq!(
            path,
            "/home/u/.local/bin:/usr/bin:/opt/x:/bin:/usr/sbin:/sbin"
        );
        assert_eq!(
            compose_child_path(&[], None),
            "/usr/bin:/bin:/usr/sbin:/sbin"
        );
    }

    #[test]
    fn resolve_takes_first_executable_file() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        std::fs::write(a.path().join("claude"), "not executable").unwrap();
        std::fs::create_dir(b.path().join("claude-dir")).unwrap();
        let hit = script(b.path(), "claude", "true");
        let path = format!("{}:{}", a.path().display(), b.path().display());
        assert_eq!(resolve_in_path("claude", &path), Some(hit));
        assert_eq!(resolve_in_path("claude-dir", &path), None, "目录不算");
        assert_eq!(resolve_in_path("", &path), None);
        assert_eq!(resolve_in_path("../claude", &path), None);
        assert_eq!(resolve_in_path("missing", &path), None);
    }

    #[test]
    fn env_is_a_whitelist_not_an_inheritance() {
        let parent = [
            ("HOME", "/home/u"),
            ("SHELL", "/bin/zsh"),
            ("LANG", "zh_CN.UTF-8"),
            ("LC_ALL", "C"),
            ("TERM", "xterm-ghostty"),
            ("PATH", "/short"),
            ("SECRET_TOKEN", "x"),
            ("PENDINGCREW_DATA_DIR", "/d"),
            ("CREWD_DATA_DIR", "/d"),
        ];
        let mut extra = BTreeMap::new();
        extra.insert("ANTHROPIC_API_KEY".to_string(), "k".to_string());
        extra.insert("supabase_secret_key".to_string(), "s".to_string());
        extra.insert("Crewd_Thing".to_string(), "t".to_string());
        extra.insert("PendingBot_Token".to_string(), "t".to_string());
        let env = build_env(parent, "/full:/usr/bin", &extra);
        let keys: Vec<&str> = env.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            [
                "ANTHROPIC_API_KEY",
                "HOME",
                "LANG",
                "LC_ALL",
                "PATH",
                "SHELL",
                "TERM"
            ]
        );
        assert_eq!(env["PATH"], "/full:/usr/bin");
        assert_eq!(
            env["TERM"], CHILD_TERM,
            "终端是我们的 Screen，不透传 crewd 所在终端的 TERM"
        );
    }

    #[test]
    fn additional_env_can_override_whitelisted_values() {
        let mut extra = BTreeMap::new();
        extra.insert("HOME".to_string(), "/elsewhere".to_string());
        let env = build_env([("HOME", "/home/u")], "/p", &extra);
        assert_eq!(env["HOME"], "/elsewhere");
    }
}
```

- [ ] **Step 3: 跑测试确认失败**

Run: `cargo test -p crewd-exec`
Expected: 编译失败，``cannot find function `extract_sentinel_path` in this scope``（以及本任务其余待实现的名字）。

- [ ] **Step 4: 实现**

放在 `crates/exec/src/env.rs` 的测试模块之前：

```rust
//! 找 agent 可执行文件、拼子进程环境（Swift `LocalCodingAgentExecutable` /
//! `LocalCodingAgentEnv` 的移植）。
//!
//! 从 launchd / systemd 拉起的进程只有一条最小 PATH，用户装在 `~/.local/bin`、
//! npm、bun 下的 `claude` 找不到。所以先起一次登录交互 shell 把它的 `$PATH` 抓回来，
//! 用同一条 PATH 定位可执行文件**并**注入子进程（否则「找得到 codex、codex 找不到
//! node」）。抓 PATH 是一次性的：调用方拿到 [`AgentPaths`] 后自己留着。
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

/// 哨兵：把真正的 `$PATH` 从交互 rc 文件往 stdout 吐的噪声里摘出来。
pub const PATH_SENTINEL_OPEN: &str = "<<<PCREW_PATH:";
pub const PATH_SENTINEL_CLOSE: &str = ":PCREW_PATH>>>";
/// 登录 shell 最多等多久（rc 文件可能卡住）。
pub const LOGIN_SHELL_TIMEOUT: Duration = Duration::from_secs(5);
/// 任何 PATH 都必须带上的系统目录（`env` / `sh` / `git` 在这里）。
pub const SYSTEM_DIRS: [&str; 4] = ["/usr/bin", "/bin", "/usr/sbin", "/sbin"];
/// 从父进程透传的环境变量（另加全部 `LC_*`）。PATH 与 TERM 不透传，见 [`build_env`]。
pub const PASSTHROUGH_KEYS: [&str; 6] = ["HOME", "SHELL", "LANG", "USER", "LOGNAME", "TMPDIR"];
/// 调用方给的额外变量里绝不放行的前缀（大小写不敏感）：我们自己的凭据与配置。
pub const FORBIDDEN_PREFIXES: [&str; 4] = ["PENDINGBOT_", "PENDINGCREW_", "SUPABASE_", "CREWD_"];
/// 子进程的 TERM。终端是我们的 `crewd_term::Screen`（xterm 系），不是 crewd 自己所在的终端。
pub const CHILD_TERM: &str = "xterm-256color";

/// 抓一次登录 shell PATH 之后的结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentPaths {
    /// 子进程用的 PATH，也是找可执行文件用的那一条。
    pub child_path: String,
}

impl AgentPaths {
    /// 起一次 `$SHELL -lic`（缺省 macOS `/bin/zsh`、其它 `/bin/sh`）抓 PATH，
    /// 再并上父进程 PATH 与系统目录。抓失败只是少了登录 shell 那几段，不报错。
    pub async fn discover() -> Self {
        let shell = std::env::var_os("SHELL")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(default_shell()));
        let login = login_shell_path(&shell, LOGIN_SHELL_TIMEOUT).await;
        let search: Vec<String> = login
            .as_deref()
            .unwrap_or("")
            .split(':')
            .map(str::to_string)
            .collect();
        let parent = std::env::var("PATH").ok();
        Self {
            child_path: compose_child_path(&search, parent.as_deref()),
        }
    }

    /// 在 `child_path` 里找第一个可执行的 `name`。
    pub fn resolve(&self, name: &str) -> Option<PathBuf> {
        resolve_in_path(name, &self.child_path)
    }
}

fn default_shell() -> &'static str {
    if cfg!(target_os = "macos") {
        "/bin/zsh"
    } else {
        "/bin/sh"
    }
}

/// 起 `shell -lic "printf '<哨兵>%s<哨兵>' "$PATH""`，取哨兵之间那段。
/// 超时、退出码非 0、没有哨兵、哨兵之间为空 → `None`。超时会杀掉 shell。
pub async fn login_shell_path(shell: &Path, timeout: Duration) -> Option<String> {
    let mut cmd = tokio::process::Command::new(shell);
    cmd.arg("-lic")
        .arg(format!(
            "/usr/bin/printf '{PATH_SENTINEL_OPEN}%s{PATH_SENTINEL_CLOSE}' \"$PATH\""
        ))
        .current_dir(std::env::temp_dir())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let out = tokio::time::timeout(timeout, cmd.output())
        .await
        .ok()?
        .ok()?;
    if !out.status.success() {
        return None;
    }
    extract_sentinel_path(&String::from_utf8_lossy(&out.stdout))
}

/// 从 shell 输出里取哨兵之间的 PATH。
pub fn extract_sentinel_path(output: &str) -> Option<String> {
    let start = output.find(PATH_SENTINEL_OPEN)? + PATH_SENTINEL_OPEN.len();
    let len = output[start..].find(PATH_SENTINEL_CLOSE)?;
    let path = &output[start..start + len];
    (!path.is_empty()).then(|| path.to_string())
}

/// 登录 shell 目录 → 父进程 PATH → 系统目录，去空白、去重、保序拼成一条。
pub fn compose_child_path(search_dirs: &[String], parent_path: Option<&str>) -> String {
    let mut out: Vec<&str> = Vec::new();
    let candidates = search_dirs
        .iter()
        .map(String::as_str)
        .chain(parent_path.unwrap_or("").split(':'))
        .chain(SYSTEM_DIRS);
    for dir in candidates {
        let dir = dir.trim();
        if !dir.is_empty() && !out.contains(&dir) {
            out.push(dir);
        }
    }
    out.join(":")
}

/// PATH 里第一个是普通文件且可执行的 `name`。`name` 含 `/` 或 NUL、或为空 → `None`。
pub fn resolve_in_path(name: &str, path: &str) -> Option<PathBuf> {
    if name.is_empty() || name.contains('/') || name.contains('\0') {
        return None;
    }
    path.split(':').filter(|d| !d.is_empty()).find_map(|dir| {
        let candidate = Path::new(dir).join(name);
        let is_file = std::fs::metadata(&candidate).is_ok_and(|m| m.is_file());
        let executable = rustix::fs::access(&candidate, rustix::fs::Access::EXEC_OK).is_ok();
        (is_file && executable).then_some(candidate)
    })
}

/// 调用方给的额外变量名是否该拒收。
pub fn is_forbidden_key(key: &str) -> bool {
    let upper = key.to_ascii_uppercase();
    FORBIDDEN_PREFIXES.iter().any(|p| upper.starts_with(p))
}

/// 子进程的完整环境（子进程**不**继承 crewd 的环境，只拿这一份）。
/// 白名单键 + `LC_*` 从 `parent` 取；PATH = `child_path`；TERM = [`CHILD_TERM`]；
/// 最后叠上 `additional`（拒收 [`FORBIDDEN_PREFIXES`]，可覆盖前面的值）。
pub fn build_env<'a>(
    parent: impl IntoIterator<Item = (&'a str, &'a str)>,
    child_path: &str,
    additional: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    for (key, value) in parent {
        if PASSTHROUGH_KEYS.contains(&key) || key.starts_with("LC_") {
            env.insert(key.to_string(), value.to_string());
        }
    }
    env.insert("PATH".to_string(), child_path.to_string());
    env.insert("TERM".to_string(), CHILD_TERM.to_string());
    for (key, value) in additional {
        if !is_forbidden_key(key) {
            env.insert(key.clone(), value.clone());
        }
    }
    env
}

/// [`build_env`] 用当前进程环境做 `parent`。非 UTF-8 的变量跳过。
pub fn build_env_from_process(
    child_path: &str,
    additional: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let parent: Vec<(String, String)> = std::env::vars_os()
        .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
        .collect();
    build_env(
        parent.iter().map(|(k, v)| (k.as_str(), v.as_str())),
        child_path,
        additional,
    )
}
```

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -p crewd-exec`
Expected: `8 passed`（`hanging_login_shell_times_out` 约 0.3s）。

- [ ] **Step 6: 全量检查并提交**

Run: `./scripts/check.sh` → 0。

```bash
git add Cargo.lock crates/exec/Cargo.toml crates/exec/src/lib.rs crates/exec/src/env.rs
git commit -m "crewd-exec: locate agent executables and build the child environment whitelist

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 2: claude 启动参数与临时文件（`launch`）

argv 的顺序、`--resume` / `--session-id` 互斥、`--permission-mode auto` 照搬 Swift `SessionConfig.argv()`；**首条 prompt 不进 argv**。世界观、settings、mcp-config 写成 `<runtime_dir>/claude-<会话号>/` 下的 0600 文件（目录 0700），`LaunchFiles` drop 时整个删掉。hooks / MCP 的命令行指向可配置的 helper（裁定 14）；fast 模式写 `"fastMode"` 并在模型不含 opus 时强制 `opus`（Swift `CrewSessionRunner` 2014 行）。

**Files:**
- Create: `crewd/crates/exec/src/launch.rs`
- Modify: `crewd/crates/exec/src/lib.rs`、`crewd/crates/exec/Cargo.toml`、`crewd/Cargo.lock`

**Interfaces:**
- Consumes: 无。
- Produces（`crewd_exec::launch`）:
  - `pub enum AgentSessionArg { Resume(String), New(String) }`，`fn id(&self) -> &str`
  - `pub struct ClaudeArgs { pub session: AgentSessionArg, pub permission_mode: String, pub system_prompt_file: Option<PathBuf>, pub settings_file: Option<PathBuf>, pub mcp_config_file: Option<PathBuf>, pub model: Option<String>, pub effort: Option<String> }`：`fn new(AgentSessionArg) -> Self`、`fn argv(&self) -> Vec<String>`
  - `fn new_agent_session_id() -> String`（小写 UUID v4）
  - `pub struct CommsSpec { pub helper: PathBuf, pub data_dir: PathBuf, pub crew_id: String, pub session_id: String, pub captain: bool, pub structured_post: bool, pub label: Option<String>, pub hooks: bool }`
  - `pub struct HelperCommands<'a>(pub &'a CommsSpec)`：`fn mcp_args(&self) -> Vec<String>`、`fn hook_command(&self, HookEvent) -> String`；`pub enum HookEvent { PostToolUse, PreToolUse, Stop }`（`ALL`、`settings_key()`）；`fn shell_quote(&str) -> String`
  - `#[derive(Default)] pub struct ClaudeSpec { pub workdir: PathBuf, pub model: Option<String>, pub effort: Option<String>, pub fast_mode: Option<bool>, pub system_prompt: Option<String>, pub initial_prompt: Option<String>, pub comms: Option<CommsSpec>, pub additional_env: BTreeMap<String, String> }`
  - `pub struct LaunchFiles`（`fn dir(&self) -> &Path`；drop 删目录）、`fn prepare_claude(spec: &ClaudeSpec, session: AgentSessionArg, runtime_dir: &Path) -> io::Result<(ClaudeArgs, LaunchFiles)>`

- [ ] **Step 1: 加依赖**

```bash
cargo add --offline -p crewd-exec serde_json uuid --features uuid/v4
```

- [ ] **Step 2: 写失败的测试**

`crates/exec/src/lib.rs` 整个换成：

```rust
//! crewd 的执行器：起 agent 进程（PTY）、读它的屏幕、投递正文、认健康状态、收尸与恢复。
//! 其余部分只经 `driver::SessionHandle` 这一个窄接口与它交互（spec §13 Q12）。
pub mod env;
pub mod launch;
```

`crates/exec/src/launch.rs`：先只放测试模块：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn comms() -> CommsSpec {
        CommsSpec {
            helper: PathBuf::from("/opt/crew d/crewd"),
            data_dir: PathBuf::from("/data"),
            crew_id: "local-aaaa".to_string(),
            session_id: "worker-1".to_string(),
            captain: true,
            structured_post: true,
            label: Some("O'Neil".to_string()),
            hooks: true,
        }
    }

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn argv_order_and_auto_mode() {
        let mut a = ClaudeArgs::new(AgentSessionArg::New("u-1".into()));
        a.system_prompt_file = Some("/r/sp.md".into());
        a.settings_file = Some("/r/s.json".into());
        a.mcp_config_file = Some("/r/m.json".into());
        a.model = Some("opus".into());
        a.effort = Some("high".into());
        assert_eq!(
            a.argv(),
            [
                "--session-id",
                "u-1",
                "--permission-mode",
                "auto",
                "--append-system-prompt-file",
                "/r/sp.md",
                "--settings",
                "/r/s.json",
                "--mcp-config",
                "/r/m.json",
                "--model",
                "opus",
                "--effort",
                "high",
            ]
        );
    }

    #[test]
    fn resume_replaces_session_id_and_empty_values_are_dropped() {
        let mut a = ClaudeArgs::new(AgentSessionArg::Resume("old".into()));
        a.model = Some(String::new());
        assert_eq!(a.argv(), ["--resume", "old", "--permission-mode", "auto"]);
    }

    #[test]
    fn new_ids_are_lowercase_uuids() {
        let id = new_agent_session_id();
        assert_eq!(id.len(), 36);
        assert_eq!(id, id.to_lowercase());
        assert_ne!(id, new_agent_session_id());
    }

    #[test]
    fn prompt_never_reaches_argv_and_files_are_private() {
        let rt = tempfile::tempdir().unwrap();
        let spec = ClaudeSpec {
            system_prompt: Some("世界观".into()),
            initial_prompt: Some("SECRET-BRIEF 第一条任务".into()),
            comms: Some(comms()),
            ..ClaudeSpec::default()
        };
        let (args, files) =
            prepare_claude(&spec, AgentSessionArg::New("u-2".into()), rt.path()).unwrap();
        let argv = args.argv().join(" ");
        assert!(
            !argv.contains("SECRET-BRIEF"),
            "首条 prompt 绝不进 argv：{argv}"
        );
        assert_eq!(mode(files.dir()), 0o700);
        for f in [
            &args.system_prompt_file,
            &args.settings_file,
            &args.mcp_config_file,
        ] {
            let f = f.as_ref().expect("三个文件都该写出来");
            assert!(f.starts_with(files.dir()));
            assert_eq!(mode(f), 0o600, "{}", f.display());
        }
        assert_eq!(
            std::fs::read_to_string(args.system_prompt_file.as_ref().unwrap()).unwrap(),
            "世界观"
        );
        let dir = files.dir().to_path_buf();
        drop(files);
        assert!(!dir.exists(), "LaunchFiles drop 时删掉整个目录");
    }

    #[test]
    fn mcp_config_is_exec_form_and_hooks_are_quoted_shell_strings() {
        let rt = tempfile::tempdir().unwrap();
        let spec = ClaudeSpec {
            comms: Some(comms()),
            ..ClaudeSpec::default()
        };
        let (args, _files) =
            prepare_claude(&spec, AgentSessionArg::New("u-3".into()), rt.path()).unwrap();
        let mcp: Value =
            serde_json::from_slice(&std::fs::read(args.mcp_config_file.unwrap()).unwrap()).unwrap();
        assert_eq!(mcp["mcpServers"]["crew"]["command"], "/opt/crew d/crewd");
        assert_eq!(
            mcp["mcpServers"]["crew"]["args"],
            json!([
                "mcp",
                "--data-dir",
                "/data",
                "--crew",
                "local-aaaa",
                "--session",
                "worker-1",
                "--agent",
                "claude",
                "--captain",
                "--structured-post",
                "--label",
                "O'Neil"
            ])
        );
        let settings: Value =
            serde_json::from_slice(&std::fs::read(args.settings_file.unwrap()).unwrap()).unwrap();
        assert_eq!(
            settings["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
            "'/opt/crew d/crewd' 'hook' 'pre-tool-use' '--data-dir' '/data' '--crew' \
             'local-aaaa' '--session' 'worker-1' '--gate' 'computer-use'"
        );
        assert_eq!(
            settings["hooks"]["Stop"][0]["hooks"][0]["command"],
            "'/opt/crew d/crewd' 'hook' 'stop' '--data-dir' '/data' '--crew' 'local-aaaa' \
             '--session' 'worker-1' '--captain' '--label' 'O'\\''Neil'"
        );
        assert_eq!(settings["hooks"]["Stop"][0]["hooks"][0]["type"], "command");
        assert!(settings.get("fastMode").is_none());
    }

    #[test]
    fn hooks_can_be_omitted_until_m3() {
        let rt = tempfile::tempdir().unwrap();
        let mut c = comms();
        c.hooks = false;
        let spec = ClaudeSpec {
            comms: Some(c),
            ..ClaudeSpec::default()
        };
        let (args, _files) =
            prepare_claude(&spec, AgentSessionArg::New("u-4".into()), rt.path()).unwrap();
        assert!(args.mcp_config_file.is_some());
        assert_eq!(
            args.settings_file, None,
            "没有 hook 也没有 fastMode 就不带 --settings"
        );
    }

    #[test]
    fn fast_mode_forces_opus_and_writes_setting() {
        let rt = tempfile::tempdir().unwrap();
        let spec = ClaudeSpec {
            model: Some("sonnet".into()),
            fast_mode: Some(true),
            ..ClaudeSpec::default()
        };
        let (args, _f) =
            prepare_claude(&spec, AgentSessionArg::New("u-5".into()), rt.path()).unwrap();
        assert_eq!(args.model.as_deref(), Some("opus"));
        let settings: Value =
            serde_json::from_slice(&std::fs::read(args.settings_file.unwrap()).unwrap()).unwrap();
        assert_eq!(settings, json!({"fastMode": true}));

        let spec = ClaudeSpec {
            model: Some("claude-opus-5".into()),
            fast_mode: Some(true),
            ..ClaudeSpec::default()
        };
        let (args, _f) =
            prepare_claude(&spec, AgentSessionArg::New("u-6".into()), rt.path()).unwrap();
        assert_eq!(
            args.model.as_deref(),
            Some("claude-opus-5"),
            "已是 opus 系就不改"
        );
    }

    #[test]
    fn preparing_the_same_id_twice_starts_clean() {
        let rt = tempfile::tempdir().unwrap();
        let spec = ClaudeSpec {
            system_prompt: Some("v1".into()),
            ..ClaudeSpec::default()
        };
        let (_a, first) =
            prepare_claude(&spec, AgentSessionArg::New("same".into()), rt.path()).unwrap();
        std::mem::forget(first);
        let spec = ClaudeSpec {
            system_prompt: Some("v2".into()),
            ..ClaudeSpec::default()
        };
        let (args, _second) =
            prepare_claude(&spec, AgentSessionArg::New("same".into()), rt.path()).unwrap();
        assert_eq!(
            std::fs::read_to_string(args.system_prompt_file.unwrap()).unwrap(),
            "v2"
        );
    }
}
```

- [ ] **Step 3: 跑测试确认失败**

Run: `cargo test -p crewd-exec launch::`
Expected: 编译失败，``cannot find function `prepare_claude` in this scope``（还有 `ClaudeSpec`、`CommsSpec` 等）。

- [ ] **Step 4: 实现**

放在 `crates/exec/src/launch.rs` 的测试模块之前：

```rust
//! claude 的启动参数与临时文件（Swift `SessionConfig.argv` / `LocalSessionLaunch` 的移植）。
//!
//! 两条铁律：
//! - **首条 prompt 绝不进 argv**：本机任何进程 `ps` 都看得见 argv（2026-08-28 出过事故）。
//!   它走 PTY 投递（[`crate::delivery`]）。
//! - `--permission-mode auto`，绝不 bypass。
//!
//! hooks / MCP 的命令行指向一个可配置的 helper（M3 实现 `crewd mcp` / `crewd hook`）。
//! 命令行的形状只在 [`HelperCommands`] 一处拼，M3 改子命令时只改这里。
use std::collections::BTreeMap;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

/// claude 用哪个会话号：续跑旧的，或用我们生成的新号（两者互斥，claude 不接受同时给）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentSessionArg {
    Resume(String),
    New(String),
}

impl AgentSessionArg {
    pub fn id(&self) -> &str {
        match self {
            Self::Resume(id) | Self::New(id) => id,
        }
    }
}

/// 一次 claude 启动的全部 argv 输入。只有值，没有文件 IO。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaudeArgs {
    pub session: AgentSessionArg,
    pub permission_mode: String,
    pub system_prompt_file: Option<PathBuf>,
    pub settings_file: Option<PathBuf>,
    pub mcp_config_file: Option<PathBuf>,
    pub model: Option<String>,
    pub effort: Option<String>,
}

impl ClaudeArgs {
    pub fn new(session: AgentSessionArg) -> Self {
        Self {
            session,
            permission_mode: "auto".to_string(),
            system_prompt_file: None,
            settings_file: None,
            mcp_config_file: None,
            model: None,
            effort: None,
        }
    }

    /// 顺序与 Swift 版一致：会话号、权限、世界观、settings、mcp、model、effort。空值不带。
    pub fn argv(&self) -> Vec<String> {
        let mut a = Vec::new();
        match &self.session {
            AgentSessionArg::Resume(id) => a.extend(["--resume".to_string(), id.clone()]),
            AgentSessionArg::New(id) => a.extend(["--session-id".to_string(), id.clone()]),
        }
        a.extend([
            "--permission-mode".to_string(),
            self.permission_mode.clone(),
        ]);
        let files = [
            ("--append-system-prompt-file", &self.system_prompt_file),
            ("--settings", &self.settings_file),
            ("--mcp-config", &self.mcp_config_file),
        ];
        for (flag, file) in files {
            if let Some(f) = file {
                a.extend([flag.to_string(), f.display().to_string()]);
            }
        }
        for (flag, value) in [("--model", &self.model), ("--effort", &self.effort)] {
            if let Some(v) = value.as_deref().filter(|v| !v.is_empty()) {
                a.extend([flag.to_string(), v.to_string()]);
            }
        }
        a
    }
}

/// 生成新会话号：小写 UUID v4（claude `--session-id` 要 UUID）。
pub fn new_agent_session_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// crew 通讯（MCP + hooks）要用的 helper 与身份。M2b 里 helper 只是一个路径参数。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommsSpec {
    /// helper 可执行文件（终态是 crewd 自己）。
    pub helper: PathBuf,
    /// crewd 数据目录（helper 据此找 socket）。
    pub data_dir: PathBuf,
    pub crew_id: String,
    /// crewd 的 session id（不是 claude 的会话号）。
    pub session_id: String,
    pub captain: bool,
    pub structured_post: bool,
    pub label: Option<String>,
    /// false = 只挂 MCP，不注册任何 hook（M3 之前 helper 还没有 `hook` 子命令）。
    pub hooks: bool,
}

/// helper 命令行。M3 定子命令时只改这里。
pub struct HelperCommands<'a>(pub &'a CommsSpec);

impl HelperCommands<'_> {
    fn identity(&self) -> Vec<String> {
        let c = self.0;
        vec![
            "--data-dir".to_string(),
            c.data_dir.display().to_string(),
            "--crew".to_string(),
            c.crew_id.clone(),
            "--session".to_string(),
            c.session_id.clone(),
        ]
    }

    /// MCP server：exec 形式（claude 直接 spawn，不过 shell），所以不加引号。
    pub fn mcp_args(&self) -> Vec<String> {
        let c = self.0;
        let mut a = vec!["mcp".to_string()];
        a.extend(self.identity());
        a.extend(["--agent".to_string(), "claude".to_string()]);
        if c.captain {
            a.push("--captain".to_string());
        }
        if c.structured_post {
            a.push("--structured-post".to_string());
        }
        if let Some(label) = c.label.as_deref().filter(|l| !l.is_empty()) {
            a.extend(["--label".to_string(), label.to_string()]);
        }
        a
    }

    /// hook：claude 经 `sh -c` 跑，所以整条按词单引号拼成一个 shell 串。
    pub fn hook_command(&self, event: HookEvent) -> String {
        let c = self.0;
        let mut words = vec![
            c.helper.display().to_string(),
            "hook".to_string(),
            event.subcommand().to_string(),
        ];
        words.extend(self.identity());
        match event {
            HookEvent::PostToolUse => {
                if c.captain {
                    words.push("--captain".to_string());
                }
            }
            HookEvent::PreToolUse => {
                words.extend(["--gate".to_string(), "computer-use".to_string()]);
            }
            HookEvent::Stop => {
                if c.captain {
                    words.push("--captain".to_string());
                }
                if let Some(label) = c.label.as_deref().filter(|l| !l.is_empty()) {
                    words.extend(["--label".to_string(), label.to_string()]);
                }
            }
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

    fn subcommand(self) -> &'static str {
        match self {
            Self::PostToolUse => "post-tool-use",
            Self::PreToolUse => "pre-tool-use",
            Self::Stop => "stop",
        }
    }
}

/// 单引号包一个 shell 词（内部的 `'` 写成 `'\''`）。
pub fn shell_quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', "'\\''"))
}

/// 一次启动要的全部输入（调用方给的）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClaudeSpec {
    pub workdir: PathBuf,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// `Some(true)`：settings 写 `"fastMode": true`，且模型不含 opus 时强制 `opus`；
    /// `Some(false)`：写 `"fastMode": false`；`None`：不写。
    pub fast_mode: Option<bool>,
    /// 世界观（M3 渲染）。写成 0600 文件走 `--append-system-prompt-file`。
    pub system_prompt: Option<String>,
    /// 开场任务。**只走 PTY 投递**，这里只是捎带给驱动层。
    pub initial_prompt: Option<String>,
    pub comms: Option<CommsSpec>,
    /// 额外环境变量（如 API key）。过 [`crate::env::build_env`] 的禁用前缀。
    pub additional_env: BTreeMap<String, String>,
}

/// 本次启动写出来的临时文件所在目录。drop 时整个删掉（会话结束就不需要了）。
#[derive(Debug)]
pub struct LaunchFiles {
    dir: PathBuf,
}

impl LaunchFiles {
    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

impl Drop for LaunchFiles {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// 写临时文件、拼 argv。文件放在 `runtime_dir/<会话号>/`（目录 0700、文件 0600）。
/// 同一会话号重复准备会先清掉旧目录。
pub fn prepare_claude(
    spec: &ClaudeSpec,
    session: AgentSessionArg,
    runtime_dir: &Path,
) -> std::io::Result<(ClaudeArgs, LaunchFiles)> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(runtime_dir)?;
    let dir = runtime_dir.join(format!("claude-{}", session.id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
    let files = LaunchFiles { dir };

    let mut args = ClaudeArgs::new(session);
    args.effort = spec.effort.clone();
    args.model = spec.model.clone();
    if spec.fast_mode == Some(true)
        && !args
            .model
            .as_deref()
            .is_some_and(|m| m.to_lowercase().contains("opus"))
    {
        args.model = Some("opus".to_string());
    }
    if let Some(text) = &spec.system_prompt {
        args.system_prompt_file = Some(write_private(
            files.dir(),
            "system-prompt.md",
            text.as_bytes(),
        )?);
    }
    let mut settings = serde_json::Map::new();
    if let Some(comms) = &spec.comms {
        let helper = HelperCommands(comms);
        let mcp = json!({"mcpServers": {"crew": {
            "command": comms.helper.display().to_string(),
            "args": helper.mcp_args(),
        }}});
        args.mcp_config_file = Some(write_json(files.dir(), "mcp.json", &mcp)?);
        if comms.hooks {
            let hooks: serde_json::Map<String, Value> = HookEvent::ALL
                .iter()
                .map(|e| {
                    let cmd = helper.hook_command(*e);
                    (
                        e.settings_key().to_string(),
                        json!([{"hooks": [{"type": "command", "command": cmd}]}]),
                    )
                })
                .collect();
            settings.insert("hooks".to_string(), Value::Object(hooks));
        }
    }
    if let Some(fast) = spec.fast_mode {
        settings.insert("fastMode".to_string(), Value::Bool(fast));
    }
    if !settings.is_empty() {
        args.settings_file = Some(write_json(
            files.dir(),
            "settings.json",
            &Value::Object(settings),
        )?);
    }
    Ok((args, files))
}

fn write_json(dir: &Path, name: &str, value: &Value) -> std::io::Result<PathBuf> {
    let bytes = serde_json::to_vec_pretty(value).map_err(std::io::Error::other)?;
    write_private(dir, name, &bytes)
}

/// 0600 新建（已存在即失败——目录是刚建的，撞上说明有人在抢）。
fn write_private(dir: &Path, name: &str, bytes: &[u8]) -> std::io::Result<PathBuf> {
    let path = dir.join(name);
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    Ok(path)
}
```

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -p crewd-exec`
Expected: `16 passed`。

- [ ] **Step 6: 全量检查并提交**

Run: `./scripts/check.sh` → 0。

```bash
git add Cargo.lock crates/exec/Cargo.toml crates/exec/src/lib.rs crates/exec/src/launch.rs
git commit -m "crewd-exec: claude argv, private launch files and helper command lines

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 3: PTY（`pty`）

裁定 2 的手写实现：rustix-openpty 开一对 PTY（带初始 winsize、IUTF8），`tokio::process::Command` 起子进程（`env_clear`、cwd、三个标准流 = 从端），`pre_exec` 里 `setsid` + `TIOCSCTTY`；主端设非阻塞挂 `AsyncFd`。`spawn` 之后立刻 drop `Command`——它攥着从端的三个副本，不放掉主端永远读不到 EOF。Linux 上从端全关后读主端报 EIO，映射成 EOF。

**Files:**
- Create: `crewd/crates/exec/src/pty.rs`
- Modify: `crewd/crates/exec/src/lib.rs`、`crewd/crates/exec/Cargo.toml`、`crewd/Cargo.lock`

**Interfaces:**
- Consumes: 无。
- Produces（`crewd_exec::pty`，只给驱动层用）:
  - `pub const CELL_PIXELS: u16 = 16`
  - `pub struct SpawnSpec { pub program: PathBuf, pub args: Vec<String>, pub env: BTreeMap<String, String>, pub cwd: PathBuf, pub cols: u16, pub lines: u16 }`
  - `pub struct PtyProcess { pub child: tokio::process::Child, pub pid: u32, pub master: PtyMaster }`；`fn spawn_pty(&SpawnSpec) -> io::Result<PtyProcess>`（必须在 tokio 运行时里调）
  - `impl PtyMaster`：`async fn read(&self, &mut [u8]) -> io::Result<usize>`（0 = 从端全关）、`async fn write_all(&self, &[u8]) -> io::Result<()>`、`fn resize(&self, cols: u16, lines: u16) -> io::Result<()>`（`&self`，可经 `Arc` 被读、写两个任务共享）

- [ ] **Step 1: 加依赖**

```bash
cargo add --offline -p crewd-exec rustix-openpty@0.2 libc
```

- [ ] **Step 2: 写失败的测试**

`crates/exec/src/lib.rs` 整个换成：

```rust
//! crewd 的执行器：起 agent 进程（PTY）、读它的屏幕、投递正文、认健康状态、收尸与恢复。
//! 其余部分只经 `driver::SessionHandle` 这一个窄接口与它交互（spec §13 Q12）。
pub mod env;
pub mod launch;
pub mod pty;
```

`crates/exec/src/pty.rs`：先只放测试模块（`resize` 那条先读到初始尺寸再改，否则改尺寸可能抢在第一次 `stty size` 之前）：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn sh(script: &str) -> SpawnSpec {
        let mut env = BTreeMap::new();
        env.insert("PATH".to_string(), "/usr/bin:/bin".to_string());
        env.insert("FOO".to_string(), "bar".to_string());
        SpawnSpec {
            program: "/bin/sh".into(),
            args: vec!["-c".to_string(), script.to_string()],
            env,
            cwd: std::env::temp_dir(),
            cols: 80,
            lines: 25,
        }
    }

    async fn read_to_end(master: &PtyMaster) -> String {
        let mut out = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            let n = tokio::time::timeout(Duration::from_secs(10), master.read(&mut buf))
                .await
                .expect("10s 内没读到 EOF")
                .unwrap();
            if n == 0 {
                return String::from_utf8_lossy(&out).into_owned();
            }
            out.extend_from_slice(&buf[..n]);
        }
    }

    #[tokio::test]
    async fn output_exit_code_and_eof() {
        let mut p = spawn_pty(&sh("printf hello; exit 3")).unwrap();
        assert!(read_to_end(&p.master).await.contains("hello"));
        assert_eq!(p.child.wait().await.unwrap().code(), Some(3));
    }

    #[tokio::test]
    async fn environment_is_replaced_not_inherited() {
        // SAFETY: 测试进程里设一个变量，确认它**不会**漏进子进程。
        unsafe { std::env::set_var("CREWD_EXEC_LEAK_PROBE", "leaked") };
        let mut p = spawn_pty(&sh("echo \"[$FOO|$HOME|$CREWD_EXEC_LEAK_PROBE]\"")).unwrap();
        let out = read_to_end(&p.master).await;
        assert!(out.contains("[bar||]"), "{out:?}");
        p.child.wait().await.unwrap();
    }

    #[tokio::test]
    async fn child_is_session_leader_with_controlling_tty() {
        let mut p = spawn_pty(&sh("echo via-tty > /dev/tty && echo CTTY-OK; sleep 1")).unwrap();
        let pgid = rustix::process::getpgid(rustix::process::Pid::from_raw(p.pid as i32)).unwrap();
        assert_eq!(
            pgid.as_raw_nonzero().get() as u32,
            p.pid,
            "setsid 后 pgid == pid"
        );
        let out = read_to_end(&p.master).await;
        assert!(
            out.contains("via-tty") && out.contains("CTTY-OK"),
            "{out:?}"
        );
        p.child.wait().await.unwrap();
    }

    #[tokio::test]
    async fn sigpipe_is_default_in_the_child() {
        let mut p = spawn_pty(&sh("yes | head -n 1; echo done")).unwrap();
        let out = read_to_end(&p.master).await;
        assert!(out.contains("done"), "{out:?}");
        assert!(
            !out.to_lowercase().contains("broken pipe"),
            "SIGPIPE 被继承成忽略了：{out:?}"
        );
        p.child.wait().await.unwrap();
    }

    /// 读到 `needle` 出现为止，返回读到的全部。
    async fn read_until(master: &PtyMaster, needle: &str) -> String {
        let mut out = Vec::new();
        let mut buf = [0u8; 4096];
        while !String::from_utf8_lossy(&out).contains(needle) {
            let n = tokio::time::timeout(Duration::from_secs(10), master.read(&mut buf))
                .await
                .unwrap_or_else(|_| panic!("10s 内没等到 {needle:?}"))
                .unwrap();
            assert!(n > 0, "没等到 {needle:?} 就 EOF 了");
            out.extend_from_slice(&buf[..n]);
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    #[tokio::test]
    async fn resize_reaches_the_child_and_input_is_written() {
        let mut p = spawn_pty(&sh("stty size; read line; echo \"got:$line\"; stty size")).unwrap();
        read_until(&p.master, "25 80").await;
        p.master.resize(101, 33).unwrap();
        p.master.write_all(b"ping\n").await.unwrap();
        let out = read_to_end(&p.master).await;
        assert!(out.contains("got:ping"), "{out:?}");
        assert!(out.contains("33 101"), "resize 后：{out:?}");
        p.child.wait().await.unwrap();
    }

    #[tokio::test]
    async fn missing_program_or_cwd_fails_to_spawn() {
        let mut spec = sh("true");
        spec.program = "/nonexistent/claude".into();
        assert!(spawn_pty(&spec).is_err());
        let mut spec = sh("true");
        spec.cwd = "/nonexistent/dir".into();
        assert!(spawn_pty(&spec).is_err());
    }
}
```

- [ ] **Step 3: 跑测试确认失败**

Run: `cargo test -p crewd-exec pty::`
Expected: 编译失败，``cannot find type `SpawnSpec` in this scope``。

- [ ] **Step 4: 实现**

放在 `crates/exec/src/pty.rs` 的测试模块之前：

```rust
//! 在 PTY 里起子进程（手写：rustix-openpty + tokio::process）。
//!
//! 子进程：`setsid`（自成会话与进程组，pgid == pid，stop 时 `killpg` 才收得干净）、
//! `TIOCSCTTY`（PTY 从端成为控制终端，`/dev/tty` 才打得开）、环境整份替换（不继承
//! crewd 的环境）、cwd 不存在即启动失败。SIGPIPE 由 std 在 exec 前复位成默认
//! （crewd 自己忽略 SIGPIPE，不复位的话子进程里 `yes | head` 会报 Broken pipe）。
//! 主端：非阻塞，挂在 tokio 的 `AsyncFd` 上读写。
use std::collections::BTreeMap;
use std::io;
use std::os::fd::{AsFd, OwnedFd};
use std::path::PathBuf;
use std::process::Stdio;

use rustix::termios::Winsize;
use tokio::io::Interest;
use tokio::io::unix::AsyncFd;

/// 报给 PTY 的每格像素（与 `crewd_term::Screen` 对 `CSI 14 t` 的回答一致：16×16）。
pub const CELL_PIXELS: u16 = 16;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpawnSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
    /// 子进程的**全部**环境。
    pub env: BTreeMap<String, String>,
    pub cwd: PathBuf,
    pub cols: u16,
    pub lines: u16,
}

/// 起好的子进程。`pid` 也是它的进程组号。
pub struct PtyProcess {
    pub child: tokio::process::Child,
    pub pid: u32,
    pub master: PtyMaster,
}

/// PTY 主端。
pub struct PtyMaster {
    fd: AsyncFd<OwnedFd>,
}

fn winsize(cols: u16, lines: u16) -> Winsize {
    Winsize {
        ws_row: lines,
        ws_col: cols,
        ws_xpixel: cols.saturating_mul(CELL_PIXELS),
        ws_ypixel: lines.saturating_mul(CELL_PIXELS),
    }
}

/// 起进程。必须在 tokio 运行时里调用（主端要注册到 reactor）。
pub fn spawn_pty(spec: &SpawnSpec) -> io::Result<PtyProcess> {
    let pty = rustix_openpty::openpty(None, Some(&winsize(spec.cols, spec.lines)))?;
    let (master, slave) = (pty.controller, pty.user);
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    if let Ok(mut t) = rustix::termios::tcgetattr(&master) {
        // 与 alacritty 一致：行编辑模式下退格按 UTF-8 字符删。
        t.input_modes.insert(rustix::termios::InputModes::IUTF8);
        let _ = rustix::termios::tcsetattr(&master, rustix::termios::OptionalActions::Now, &t);
    }

    let mut cmd = tokio::process::Command::new(&spec.program);
    cmd.args(&spec.args)
        .env_clear()
        .envs(&spec.env)
        .current_dir(&spec.cwd)
        .stdin(Stdio::from(slave.try_clone()?))
        .stdout(Stdio::from(slave.try_clone()?))
        .stderr(Stdio::from(slave));
    // SAFETY: 闭包只做 async-signal-safe 的裸系统调用（setsid、ioctl），不分配内存。
    unsafe {
        cmd.pre_exec(|| {
            rustix::process::setsid()?;
            rustix::process::ioctl_tiocsctty(rustix::stdio::stdin())?;
            Ok(())
        });
    }
    let child = cmd.spawn()?;
    // `cmd` 里还攥着从端的三个副本；不 drop 掉，子进程退出后主端永远读不到 EOF。
    drop(cmd);
    let pid = child
        .id()
        .ok_or_else(|| io::Error::other("子进程刚起就已被回收"))?;
    rustix::io::ioctl_fionbio(&master, true)?;
    let fd = AsyncFd::with_interest(master, Interest::READABLE | Interest::WRITABLE)?;
    Ok(PtyProcess {
        child,
        pid,
        master: PtyMaster { fd },
    })
}

impl PtyMaster {
    /// 读一块。`Ok(0)` = 从端全关了（子进程退出；Linux 上这时 read 报 EIO，也映射成 0）。
    pub async fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            let mut guard = self.fd.readable().await?;
            match guard
                .try_io(|fd| rustix::io::read(fd.get_ref(), &mut *buf).map_err(io::Error::from))
            {
                Ok(Ok(n)) => return Ok(n),
                Ok(Err(e)) if e.raw_os_error() == Some(libc::EIO) => return Ok(0),
                Ok(Err(e)) if e.kind() == io::ErrorKind::Interrupted => continue,
                Ok(Err(e)) => return Err(e),
                Err(_would_block) => continue,
            }
        }
    }

    /// 写完整段。子进程不读时会一直等（PTY 缓冲满）。
    pub async fn write_all(&self, mut bytes: &[u8]) -> io::Result<()> {
        while !bytes.is_empty() {
            let mut guard = self.fd.writable().await?;
            match guard.try_io(|fd| rustix::io::write(fd.get_ref(), bytes).map_err(io::Error::from))
            {
                Ok(Ok(0)) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(Ok(n)) => bytes = &bytes[n..],
                Ok(Err(e)) if e.kind() == io::ErrorKind::Interrupted => {}
                Ok(Err(e)) => return Err(e),
                Err(_would_block) => {}
            }
        }
        Ok(())
    }

    /// 改窗口尺寸（内核随后给前台进程组发 SIGWINCH）。
    pub fn resize(&self, cols: u16, lines: u16) -> io::Result<()> {
        rustix::termios::tcsetwinsize(self.fd.get_ref().as_fd(), winsize(cols, lines))?;
        Ok(())
    }
}
```

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -p crewd-exec`
Expected: `22 passed`（pty 那几条约 1s，`child_is_session_leader_with_controlling_tty` 里有 `sleep 1`）。

- [ ] **Step 6: 验这把尺子会红**

在 `pre_exec` 闭包里 `rustix::process::setsid()?;` 下面临时加一行 `unsafe { libc::signal(libc::SIGPIPE, libc::SIG_IGN) };` → `cargo test -p crewd-exec --lib pty` → `sigpipe_is_default_in_the_child` 红（另有一条 unnecessary `unsafe` 警告，无妨）。改回，再跑一次全绿。

已知边界（写计划时实测）：在 macOS 上**删掉 `TIOCSCTTY` 那行，`child_is_session_leader_with_controlling_tty` 照样绿**——XNU 下 `setsid` 之后子进程照样拿得到控制终端（`ps -o tty=` 也显示 ttys）。这条测试真正守的是 Linux（那边不设 `TIOCSCTTY` 打不开 `/dev/tty`）；别把它在 macOS 上的绿当成「`TIOCSCTTY` 被验过了」。

- [ ] **Step 7: 全量检查并提交**

Run: `./scripts/check.sh` → 0。

```bash
git add Cargo.lock crates/exec/Cargo.toml crates/exec/src/lib.rs crates/exec/src/pty.rs
git commit -m "crewd-exec: hand-rolled PTY spawn (setsid, controlling tty, env_clear) with async master

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 4: 投递状态机（`delivery`）

Swift `StartupPromptDelivery` 的移植，纯值、时间由调用方传入。判据全部作用在渲染完的那屏上：就绪 = 有输入行且**连续 `ready_settle`（300ms）没断**（裁定 4）；落地 = 那一行从原样变成非空的别的东西，最早 `submit_gap`（0.5s）后才作数；提交 = 发回车后那一行又变了（或被长正文顶出屏幕的提示符重新出现）。每步最多 3 次，`verify_window`（2.5s）没变就重来。等就绪期间的两种坏消息只报不终止（裁定 5）。`dialog_stable` 直接引 `crewd_term::STABLE_WINDOW`，不另写一个 3。

**Files:**
- Create: `crewd/crates/exec/src/delivery.rs`
- Modify: `crewd/crates/exec/src/lib.rs`、`crewd/crates/exec/Cargo.toml`、`crewd/Cargo.lock`

**Interfaces:**
- Consumes: `crewd_term::STABLE_WINDOW: Duration`（M2a）。
- Produces（`crewd_exec::delivery`）:
  - `pub enum Subject { Brief, Steer }`（`Copy + Eq`）
  - `pub struct Timing { pub readiness_deadline, pub verify_window, pub submit_gap: Duration, pub max_attempts: u32, pub dialog_stable, pub ready_settle: Duration }`，`Default` = 90s / 2.5s / 0.5s / 3 / `STABLE_WINDOW` / 300ms
  - `pub struct Observation<'a> { pub input_row: Option<&'a str>, pub body_visible: bool, pub dialog_present: bool, pub now: Instant }`
  - `pub enum Step { Idle, WriteBody { attempt: u32 }, Submit { attempt: u32 }, BlockedByDialog(String), NotReady(String), Undelivered(String), Unsubmitted(String), Delivered }`
  - `pub struct Delivery`：`fn new(Timing, Subject, started_at: Instant) -> Self`、`fn subject(&self) -> Subject`、`fn is_finished(&self) -> bool`、`fn next_deadline(&self) -> Option<Instant>`、`fn step(&mut self, Observation<'_>) -> Step`
  - `fn landed(row: &str, baseline: &str) -> bool`

- [ ] **Step 1: 加依赖**

```bash
cargo add --offline -p crewd-exec crewd-term --path crates/term
```

- [ ] **Step 2: 写失败的测试**

`crates/exec/src/lib.rs` 整个换成：

```rust
//! crewd 的执行器：起 agent 进程（PTY）、读它的屏幕、投递正文、认健康状态、收尸与恢复。
//! 其余部分只经 `driver::SessionHandle` 这一个窄接口与它交互（spec §13 Q12）。
pub mod delivery;
pub mod env;
pub mod launch;
pub mod pty;
```

`crates/exec/src/delivery.rs`：先只放测试模块（前 11 条照搬 Swift `StartupPromptDeliveryPlanTests`，测试用的 `Timing` 把 `ready_settle` 设成 0；`input_row_must_settle_before_anything_is_written` 单独钉裁定 4）：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn timing() -> Timing {
        Timing {
            readiness_deadline: Duration::from_secs(10),
            verify_window: Duration::from_secs(2),
            submit_gap: Duration::from_millis(500),
            max_attempts: 3,
            dialog_stable: Duration::from_secs(3),
            ready_settle: Duration::ZERO,
        }
    }

    struct Clock(Instant);

    impl Clock {
        fn at(&self, secs: f64) -> Instant {
            self.0 + Duration::from_secs_f64(secs)
        }
    }

    fn obs(row: Option<&str>, at: Instant) -> Observation<'_> {
        Observation {
            input_row: row,
            body_visible: false,
            dialog_present: false,
            now: at,
        }
    }

    fn setup() -> (Delivery, Clock) {
        let t0 = Instant::now();
        (Delivery::new(timing(), Subject::Brief, t0), Clock(t0))
    }

    #[test]
    fn nothing_is_written_before_the_input_row_exists() {
        let (mut d, c) = setup();
        for s in 0..10 {
            assert_eq!(d.step(obs(None, c.at(s as f64))), Step::Idle, "第 {s}s");
        }
    }

    #[test]
    fn not_ready_is_reported_once_and_delivery_continues() {
        let (mut d, c) = setup();
        assert_eq!(d.step(obs(None, c.at(9.0))), Step::Idle);
        match d.step(obs(None, c.at(10.0))) {
            Step::NotReady(detail) => assert!(detail.contains("开场任务"), "{detail}"),
            other => panic!("过了期限必须报，实际 {other:?}"),
        }
        assert_eq!(
            d.step(obs(None, c.at(20.0))),
            Step::Idle,
            "同一条不许反复报"
        );
        assert!(!d.is_finished(), "报了不等于放弃");
        assert_eq!(
            d.step(obs(Some(""), c.at(30.0))),
            Step::WriteBody { attempt: 1 }
        );
    }

    #[test]
    fn happy_path_writes_verifies_then_submits() {
        let (mut d, c) = setup();
        assert_eq!(
            d.step(obs(Some(""), c.at(0.5))),
            Step::WriteBody { attempt: 1 }
        );
        assert_eq!(d.step(obs(Some(""), c.at(1.0))), Step::Idle);
        let pasted = "[Pasted text #1 +212 lines]";
        assert_eq!(
            d.step(obs(Some(pasted), c.at(1.5))),
            Step::Submit { attempt: 1 }
        );
        assert_eq!(d.step(obs(Some(pasted), c.at(2.0))), Step::Idle);
        assert_eq!(d.step(obs(Some(""), c.at(2.5))), Step::Delivered);
        assert!(d.is_finished());
        assert_eq!(d.step(obs(Some(""), c.at(3.0))), Step::Idle);
    }

    #[test]
    fn a_change_right_after_writing_does_not_count_as_landed() {
        let (mut d, c) = setup();
        assert_eq!(
            d.step(obs(Some(""), c.at(0.0))),
            Step::WriteBody { attempt: 1 }
        );
        let hint = "Try \"how do I log an error?\"";
        assert_eq!(d.step(obs(Some(hint), c.at(0.2))), Step::Idle);
        assert_eq!(d.step(obs(Some(hint), c.at(0.49))), Step::Idle);
        assert_eq!(
            d.step(obs(Some("[Pasted text #1 +212 lines]"), c.at(0.5))),
            Step::Submit { attempt: 1 }
        );
    }

    #[test]
    fn landed_means_changed_and_non_empty() {
        assert!(!landed("", ""));
        assert!(!landed("Try \"fix\"", "Try \"fix\""));
        assert!(landed("[Pasted text #1 +212 lines]", ""));
        assert!(landed("修一条 bug", "Try \"fix\""));
    }

    #[test]
    fn body_that_never_lands_is_retried_then_reported() {
        let (mut d, c) = setup();
        assert_eq!(
            d.step(obs(Some(""), c.at(0.0))),
            Step::WriteBody { attempt: 1 }
        );
        assert_eq!(d.step(obs(Some(""), c.at(1.9))), Step::Idle);
        assert_eq!(
            d.step(obs(Some(""), c.at(2.0))),
            Step::WriteBody { attempt: 2 }
        );
        assert_eq!(
            d.step(obs(Some(""), c.at(4.0))),
            Step::WriteBody { attempt: 3 }
        );
        match d.step(obs(Some(""), c.at(6.0))) {
            Step::Undelivered(detail) => assert!(detail.contains("3 次"), "{detail}"),
            other => panic!("到上限必须报，实际 {other:?}"),
        }
        assert!(d.is_finished());
    }

    #[test]
    fn enter_that_is_never_accepted_is_retried_then_reported() {
        let (mut d, c) = setup();
        assert_eq!(
            d.step(obs(Some(""), c.at(0.0))),
            Step::WriteBody { attempt: 1 }
        );
        assert_eq!(
            d.step(obs(Some("brief"), c.at(0.5))),
            Step::Submit { attempt: 1 }
        );
        assert_eq!(
            d.step(obs(Some("brief"), c.at(2.5))),
            Step::Submit { attempt: 2 }
        );
        assert_eq!(
            d.step(obs(Some("brief"), c.at(4.5))),
            Step::Submit { attempt: 3 }
        );
        match d.step(obs(Some("brief"), c.at(6.5))) {
            Step::Unsubmitted(detail) => assert!(detail.contains("没提交"), "{detail}"),
            other => panic!("到上限必须报，实际 {other:?}"),
        }
        assert!(d.is_finished());
    }

    #[test]
    fn input_row_vanishing_mid_redraw_is_not_a_failure() {
        let (mut d, c) = setup();
        assert_eq!(
            d.step(obs(Some(""), c.at(0.0))),
            Step::WriteBody { attempt: 1 }
        );
        assert_eq!(d.step(obs(None, c.at(3.0))), Step::Idle);
        assert_eq!(
            d.step(obs(Some(""), c.at(3.1))),
            Step::WriteBody { attempt: 2 }
        );
    }

    #[test]
    fn long_body_that_pushed_the_prompt_off_screen_is_submitted() {
        let (mut d, c) = setup();
        assert_eq!(
            d.step(obs(Some(""), c.at(0.0))),
            Step::WriteBody { attempt: 1 }
        );
        let mut o = obs(None, c.at(0.5));
        assert_eq!(d.step(o), Step::Idle, "没有正文证据时按重绘处理");
        o.body_visible = true;
        o.now = c.at(0.6);
        assert_eq!(d.step(o), Step::Submit { attempt: 1 });
        // 提示符重新出现 = 提交了。
        assert_eq!(d.step(obs(Some(""), c.at(1.0))), Step::Delivered);
    }

    #[test]
    fn dialog_blocks_reports_once_and_does_not_finish() {
        let (mut d, c) = setup();
        let dialog = |at| Observation {
            input_row: None,
            body_visible: false,
            dialog_present: true,
            now: at,
        };
        assert_eq!(d.step(dialog(c.at(0.0))), Step::Idle);
        assert_eq!(d.step(dialog(c.at(2.9))), Step::Idle);
        match d.step(dialog(c.at(3.0))) {
            Step::BlockedByDialog(detail) => assert!(detail.contains("需要人回答"), "{detail}"),
            other => panic!("对话框稳住后必须报，实际 {other:?}"),
        }
        assert_eq!(d.step(dialog(c.at(5.0))), Step::Idle);
        assert!(!d.is_finished(), "答完之后还要补投");
    }

    #[test]
    fn waiting_on_a_dialog_does_not_count_towards_not_ready() {
        let (mut d, c) = setup();
        for s in 0..=30 {
            let step = d.step(Observation {
                input_row: None,
                body_visible: false,
                dialog_present: true,
                now: c.at(s as f64),
            });
            assert!(
                !matches!(step, Step::NotReady(_)),
                "等人期间不该报没就绪（第 {s}s）"
            );
        }
        assert_eq!(
            d.step(obs(Some(""), c.at(31.0))),
            Step::WriteBody { attempt: 1 }
        );
    }

    #[test]
    fn steer_wording_differs_but_rules_do_not() {
        let t0 = Instant::now();
        let mut d = Delivery::new(timing(), Subject::Steer, t0);
        d.step(obs(Some(""), t0));
        for s in [2.0, 4.0] {
            d.step(obs(Some(""), t0 + Duration::from_secs_f64(s)));
        }
        match d.step(obs(Some(""), t0 + Duration::from_secs(6))) {
            Step::Undelivered(detail) => {
                assert!(detail.starts_with("这条指令"), "{detail}");
                assert!(detail.contains("需要有人重发"), "{detail}");
            }
            other => panic!("{other:?}"),
        }
    }

    /// 录下来的信任框首屏画到一半时 `❯  No, exit` 单独在屏、像一行输入框：
    /// 输入行必须连续在屏 `ready_settle` 才算就绪，中途断一下就重新计。
    #[test]
    fn input_row_must_settle_before_anything_is_written() {
        let t0 = Instant::now();
        let at = |s: f64| t0 + Duration::from_secs_f64(s);
        let mut d = Delivery::new(
            Timing {
                ready_settle: Duration::from_millis(300),
                ..timing()
            },
            Subject::Brief,
            t0,
        );
        assert_eq!(d.step(obs(Some("No, exit"), at(0.0))), Step::Idle);
        assert_eq!(d.next_deadline(), Some(at(0.3)));
        assert_eq!(d.step(obs(Some("No, exit"), at(0.2))), Step::Idle);
        let dialog = Observation {
            input_row: None,
            body_visible: false,
            dialog_present: true,
            now: at(0.25),
        };
        assert_eq!(d.step(dialog), Step::Idle, "页脚画出来了：其实是对话框");
        assert_eq!(d.step(obs(Some(""), at(5.0))), Step::Idle, "答完后重新计");
        assert_eq!(d.step(obs(None, at(5.1))), Step::Idle);
        assert_eq!(
            d.step(obs(Some(""), at(5.2))),
            Step::Idle,
            "断过一次再重新计"
        );
        assert_eq!(
            d.step(obs(Some(""), at(5.5))),
            Step::WriteBody { attempt: 1 }
        );
    }

    #[test]
    fn next_deadline_tracks_the_phase() {
        let (mut d, c) = setup();
        assert_eq!(
            d.next_deadline(),
            Some(c.at(10.0)),
            "等就绪：到期报 NotReady"
        );
        d.step(obs(Some(""), c.at(1.0)));
        assert_eq!(d.next_deadline(), Some(c.at(1.5)), "写完：submit_gap 先到");
        d.step(obs(Some("x"), c.at(1.5)));
        assert_eq!(d.next_deadline(), Some(c.at(3.5)), "提交：verify_window");
        d.step(obs(Some(""), c.at(2.0)));
        assert_eq!(d.next_deadline(), None);
    }
}
```

- [ ] **Step 3: 跑测试确认失败**

Run: `cargo test -p crewd-exec delivery::`
Expected: 编译失败，``cannot find type `Timing` in this scope``。

- [ ] **Step 4: 实现**

放在 `crates/exec/src/delivery.rs` 的测试模块之前：

```rust
//! 把一段正文投进 claude 输入框的状态机（Swift `StartupPromptDelivery` 的移植）。
//!
//! 判据全部作用在**渲染完的那屏**上，不赌时序：
//! 1. 就绪 = 屏上有输入行（`crewd_term::input_row` 非 `None`），且连续 `ready_settle` 没断、
//!    其间没有对话框。录下来的信任框首屏（`tui-claude-trust.bin`）画到一半时，
//!    `❯  No, exit` 那一行单独在屏、页脚还没画出来，那一刻它长得就是输入行——
//!    不等它稳住就会把正文当按键敲进信任框。
//! 2. 落地 = 写完正文后那一行从原样变成非空的别的东西（claude 会把大段输入折成
//!    `[Pasted text #1 +N lines]`，所以不逐字比）；最早 `submit_gap` 之后才作数。
//! 3. 提交 = 发回车后那一行又变了（或提示符被长正文顶出屏幕后重新出现）。
//!
//! 每步有上限，到顶给出终局裁决。等就绪期间的两种坏消息（被对话框挡住、等不到输入框）
//! **只报不终止**：答完对话框 / 输入框终于画出来，照样投出去。
//! 纯值、时间由调用方传入——整台机器可以脱离进程单测。
use std::time::{Duration, Instant};

use crewd_term::STABLE_WINDOW;

/// 这一笔是什么。只影响裁决措辞，不影响任何判据。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Subject {
    /// 开场任务。
    Brief,
    /// 运行中的一条指令（唤醒 / 续跑 / steer）。
    Steer,
}

impl Subject {
    fn noun(self) -> &'static str {
        match self {
            Self::Brief => "开场任务",
            Self::Steer => "这条指令",
        }
    }

    fn lost_tail(self) -> &'static str {
        match self {
            Self::Brief => "这个 session 一个字都没收到，派给它的活等于没派出去——请改派或重起。",
            Self::Steer => "session 本身是好的、屏幕上也有字，但它没收到这条——需要有人重发。",
        }
    }
}

/// 上限。缺省值的理由见 Swift 版注释；调小只该发生在测试里。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timing {
    /// 等输入框就绪多久后报一次 `NotReady`（报完继续等）。
    pub readiness_deadline: Duration,
    /// 一次写入之后最多等多久回读判定。
    pub verify_window: Duration,
    /// 写完正文后最早多久才认「落地」（刚写完那一帧属于写之前；正文与回车也要分开到达）。
    pub submit_gap: Duration,
    /// 写正文 / 发回车各自最多几次。
    pub max_attempts: u32,
    /// 对话框稳定多久才认——与待决策跟踪器同一个常量。
    pub dialog_stable: Duration,
    /// 输入行要连续在屏多久才算就绪（挡住「对话框画到一半像输入行」）。
    pub ready_settle: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            readiness_deadline: Duration::from_secs(90),
            verify_window: Duration::from_millis(2500),
            submit_gap: Duration::from_millis(500),
            max_attempts: 3,
            dialog_stable: STABLE_WINDOW,
            ready_settle: Duration::from_millis(300),
        }
    }
}

/// 一拍观测量，全部来自权威屏幕。
#[derive(Clone, Copy, Debug)]
pub struct Observation<'a> {
    /// 输入行提示符后的内容；`None` = 屏上没有输入行。
    pub input_row: Option<&'a str>,
    /// 正文末尾是否已出现在屏上（长正文会把提示符顶出屏幕）。
    pub body_visible: bool,
    /// 屏上有需要人回答的对话框。
    pub dialog_present: bool,
    pub now: Instant,
}

/// 这一拍该做什么。调用方只执行，不判断。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    Idle,
    /// 写正文（不带回车）。`attempt > 1` 时先发 Ctrl-U 清掉输入行。
    WriteBody {
        attempt: u32,
    },
    /// 发回车（经空格 + 退格 + 回车三拍，见 `crate::session`）。
    Submit {
        attempt: u32,
    },
    /// 对话框挡着，没投。只报一次，**不终止**。
    BlockedByDialog(String),
    /// 等到期限也没见到输入框。只报一次，**不终止**。
    NotReady(String),
    /// 终局：正文反复没进输入行。
    Undelivered(String),
    /// 终局：正文进去了，回车反复没被接受。
    Unsubmitted(String),
    /// 终局：送达并提交。
    Delivered,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Phase {
    AwaitingReady,
    Writing {
        attempt: u32,
        not_before: Instant,
        deadline: Instant,
        baseline: String,
    },
    /// `submitted == None`：提示符已被长正文顶出屏幕，提交成功 = 输入行重新出现。
    Submitting {
        attempt: u32,
        deadline: Instant,
        submitted: Option<String>,
    },
    Finished,
}

#[derive(Clone, Debug)]
pub struct Delivery {
    timing: Timing,
    subject: Subject,
    phase: Phase,
    /// 等就绪的起点；被对话框挡住期间往后推（等人不算「没就绪」）。
    readiness_since: Instant,
    dialog_since: Option<Instant>,
    /// 输入行这一段连续在屏的起点。
    ready_since: Option<Instant>,
    reported_dialog: bool,
    reported_not_ready: bool,
}

/// 「正文真的进了输入行」：非空且不同于写之前。
pub fn landed(row: &str, baseline: &str) -> bool {
    !row.is_empty() && row != baseline
}

impl Delivery {
    pub fn new(timing: Timing, subject: Subject, started_at: Instant) -> Self {
        Self {
            timing,
            subject,
            phase: Phase::AwaitingReady,
            readiness_since: started_at,
            dialog_since: None,
            ready_since: None,
            reported_dialog: false,
            reported_not_ready: false,
        }
    }

    pub fn subject(&self) -> Subject {
        self.subject
    }

    /// 已给出终局裁决（成功或放弃）。
    pub fn is_finished(&self) -> bool {
        self.phase == Phase::Finished
    }

    /// 下一个「到点就会有变化」的时刻（驱动层据此定下一次唤醒）。`None` = 只等画面变化。
    pub fn next_deadline(&self) -> Option<Instant> {
        match &self.phase {
            Phase::AwaitingReady => {
                if let Some(since) = self.dialog_since {
                    (!self.reported_dialog).then_some(since + self.timing.dialog_stable)
                } else if let Some(since) = self.ready_since {
                    Some(since + self.timing.ready_settle)
                } else {
                    (!self.reported_not_ready)
                        .then_some(self.readiness_since + self.timing.readiness_deadline)
                }
            }
            Phase::Writing {
                not_before,
                deadline,
                ..
            } => Some((*not_before).min(*deadline)),
            Phase::Submitting { deadline, .. } => Some(*deadline),
            Phase::Finished => None,
        }
    }

    pub fn step(&mut self, obs: Observation<'_>) -> Step {
        let now = obs.now;
        let t = self.timing;
        match self.phase.clone() {
            Phase::Finished => Step::Idle,
            Phase::AwaitingReady => {
                if obs.dialog_present {
                    self.ready_since = None;
                    let since = *self.dialog_since.get_or_insert(now);
                    self.readiness_since = now;
                    if now.saturating_duration_since(since) < t.dialog_stable
                        || self.reported_dialog
                    {
                        return Step::Idle;
                    }
                    self.reported_dialog = true;
                    return Step::BlockedByDialog(format!(
                        "{}没投出去：屏幕上摆着一个需要人回答的对话框（信任此文件夹 / 命令审批这类），\
                         正文投进去会被当成按键吃掉。看一眼屏幕、代答掉它，答完会自己补投出去。",
                        self.subject.noun()
                    ));
                }
                self.dialog_since = None;
                if let Some(row) = obs.input_row {
                    let since = *self.ready_since.get_or_insert(now);
                    if now.saturating_duration_since(since) < t.ready_settle {
                        return Step::Idle;
                    }
                    self.phase = Phase::Writing {
                        attempt: 1,
                        not_before: now + t.submit_gap,
                        deadline: now + t.verify_window,
                        baseline: row.to_string(),
                    };
                    return Step::WriteBody { attempt: 1 };
                }
                self.ready_since = None;
                if now.saturating_duration_since(self.readiness_since) < t.readiness_deadline
                    || self.reported_not_ready
                {
                    return Step::Idle;
                }
                self.reported_not_ready = true;
                Step::NotReady(format!(
                    "{}还没投出去：TUI 在吐输出，但等了 {}s 也没在画面上等到输入框。看一眼它停在哪一屏。",
                    self.subject.noun(),
                    t.readiness_deadline.as_secs()
                ))
            }
            Phase::Writing {
                attempt,
                not_before,
                deadline,
                baseline,
            } => {
                let Some(row) = obs.input_row else {
                    // 重绘到一半输入行暂时不在——等下一拍；除非正文尾巴已经清楚可见
                    // （长正文把提示符顶出了屏幕），那就是落地了。
                    if now < not_before || !obs.body_visible {
                        return Step::Idle;
                    }
                    self.phase = Phase::Submitting {
                        attempt: 1,
                        deadline: now + t.verify_window,
                        submitted: None,
                    };
                    return Step::Submit { attempt: 1 };
                };
                if now >= not_before && landed(row, &baseline) {
                    self.phase = Phase::Submitting {
                        attempt: 1,
                        deadline: now + t.verify_window,
                        submitted: Some(row.to_string()),
                    };
                    return Step::Submit { attempt: 1 };
                }
                if now < deadline {
                    return Step::Idle;
                }
                if attempt >= t.max_attempts {
                    self.phase = Phase::Finished;
                    return Step::Undelivered(format!(
                        "{}没送到：正文写了 {attempt} 次，回读屏幕确认输入行始终没变（重绘把它吞了）。{}",
                        self.subject.noun(),
                        self.subject.lost_tail()
                    ));
                }
                self.phase = Phase::Writing {
                    attempt: attempt + 1,
                    not_before: now + t.submit_gap,
                    deadline: now + t.verify_window,
                    baseline,
                };
                Step::WriteBody {
                    attempt: attempt + 1,
                }
            }
            Phase::Submitting {
                attempt,
                deadline,
                submitted,
            } => {
                let Some(row) = obs.input_row else {
                    return Step::Idle;
                };
                if submitted.as_deref() != Some(row) {
                    self.phase = Phase::Finished;
                    return Step::Delivered;
                }
                if now < deadline {
                    return Step::Idle;
                }
                if attempt >= t.max_attempts {
                    self.phase = Phase::Finished;
                    return Step::Unsubmitted(format!(
                        "{}卡在输入框里没提交：正文已经进去了，但回车发了 {attempt} 次都没被接受\
                         （多半有个模态挡在前面）。输入框里那段没人敲过的字就是它——代答掉挡路的框，\
                         或按 Ctrl-U 清掉再重发。",
                        self.subject.noun()
                    ));
                }
                self.phase = Phase::Submitting {
                    attempt: attempt + 1,
                    deadline: now + t.verify_window,
                    submitted,
                };
                Step::Submit {
                    attempt: attempt + 1,
                }
            }
        }
    }
}
```

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -p crewd-exec`
Expected: `36 passed`。

- [ ] **Step 6: 全量检查并提交**

Run: `./scripts/check.sh` → 0。

```bash
git add Cargo.lock crates/exec/Cargo.toml crates/exec/src/lib.rs crates/exec/src/delivery.rs
git commit -m "crewd-exec: screen-verified prompt delivery state machine

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 5: 健康判定（`health`）

进程活着、agent 已经干不了活的几种情况，只报能可靠拿到的事实。全部是纯函数或小状态机，时间由调用方传入；唯一的 IO 是 `AuthProbe::read` 读 claude 自己的结构化日志：`projects/*/<会话号>.jsonl` 末尾 256KB，只认本次启动以来、本会话、非 sidechain 的 `assistant` 记录，最新那条是 `isApiErrorMessage + error == "authentication_failed"` 就是要登录，正常回答（`msg_` 开头、有模型名、`content` 是数组）就是已认证，其余一律「不知道」——读不到日志不等于没登录。拉起自检的判定是 Swift `SessionLaunchProbe.verdict` 原样移植（「还没生出来」不等于「死了」）。

**Files:**
- Create: `crewd/crates/exec/src/health.rs`
- Modify: `crewd/crates/exec/src/lib.rs`、`crewd/crates/exec/Cargo.toml`、`crewd/Cargo.lock`

**Interfaces:**
- Consumes: `crewd_term::{AnsiPlainTextTail, RATE_LIMIT_MENU_PHRASES}`。
- Produces（`crewd_exec::health`）:
  - `pub enum HealthKind { AuthRequired, UsageLimit, RateLimited, LaunchFailed, BriefUndelivered, PromptUndelivered }`，`fn is_quota(self) -> bool`
  - `pub struct Health { pub kind: HealthKind, pub detail: String, pub launch_verdict: Option<LaunchVerdict> }`，`fn new(HealthKind, impl Into<String>) -> Self`
  - `QUOTA_PHRASES: [&str; 6]`、`#[derive(Default)] pub struct QuotaScanner`：`fn feed(&mut self, &[u8]) -> Option<Health>`、`fn rearm(&mut self)`
  - `RATE_LIMIT_COOLDOWN: Duration`（30s）、`pub struct RateLimitScanner`（`Default`、`with_cooldown`）：`fn feed(&mut self, &[u8], now: Instant) -> bool`
  - `QUOTA_WORK_STREAK: Duration`（6s）、`fn quota_recovered(health: Option<&Health>, quota_at: Option<Instant>, working_since: Option<Instant>, now: Instant) -> bool`
  - `pub enum AuthState { Unknown, Authenticated, AuthenticationRequired }`，`fn apply(self, Option<Health>) -> Option<Health>`；`AUTH_POLL_INTERVAL: Duration`（2s）
  - `#[derive(Clone)] pub struct AuthProbe { pub session_id: String, pub projects_dir: PathBuf, pub since: SystemTime }`：`const MAX_BYTES: u64`、`fn projects_dir_for(config_dir: Option<&str>, home: Option<&str>, workdir: &Path) -> Option<PathBuf>`、`fn read(&self) -> AuthState`；`fn auth_state_in(data: &[u8], session_id: &str, since: SystemTime) -> AuthState`
  - `FIRST_OUTPUT_DEADLINE: Duration`（25s）、`LAUNCH_POLL_INTERVAL`、`pub enum LaunchVerdict { Pending, SpawnFailed, DiedSilently, Stalled, Alive }`（`is_terminal()`）、`fn launch_verdict(spawned, process_alive, ever_alive, saw_output: bool, elapsed, deadline: Duration) -> LaunchVerdict`、`fn launch_failure(LaunchVerdict, deadline: Duration, underlying: Option<&str>) -> Option<Health>`

- [ ] **Step 1: 加依赖**

`chrono` 关掉默认特性（只要解析 RFC 3339 和 `SystemTime` 互转；`Utc::now()` 用不了，测试里用 `DateTime::<Utc>::from(SystemTime::now())`）：

```bash
cargo add --offline -p crewd-exec chrono --no-default-features --features std
```

- [ ] **Step 2: 写失败的测试**

`crates/exec/src/lib.rs` 整个换成：

```rust
//! crewd 的执行器：起 agent 进程（PTY）、读它的屏幕、投递正文、认健康状态、收尸与恢复。
//! 其余部分只经 `driver::SessionHandle` 这一个窄接口与它交互（spec §13 Q12）。
pub mod delivery;
pub mod env;
pub mod health;
pub mod launch;
pub mod pty;
```

`crates/exec/src/health.rs`：先只放测试模块（认证记录的形状照搬 Swift `SessionHealthTests` 的 `authRecord`）：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn secs(s: u64) -> Duration {
        Duration::from_secs(s)
    }

    #[test]
    fn quota_phrase_fires_once_until_rearmed() {
        let mut q = QuotaScanner::default();
        assert_eq!(q.feed(b"working on it...\r\n"), None);
        let h = q
            .feed(b"\x1b[31mYou've reached\x1b[0m your usage limit")
            .unwrap();
        assert_eq!(h.kind, HealthKind::UsageLimit);
        assert_eq!(
            q.feed(b"You've reached your usage limit"),
            None,
            "哑掉直到重新武装"
        );
        q.rearm();
        assert_eq!(
            q.feed(b"still fine"),
            None,
            "重新武装时尾窗清空，旧短语不再命中"
        );
        assert!(
            q.feed("You\u{2019}ve reached your limit".as_bytes())
                .is_some()
        );
    }

    #[test]
    fn quota_phrase_split_across_chunks_matches() {
        let mut q = QuotaScanner::default();
        assert_eq!(q.feed(b"Credit bal"), None);
        assert!(q.feed(b"ance is too low").is_some());
    }

    #[test]
    fn login_documentation_quoted_in_output_is_not_a_health_event() {
        let mut q = QuotaScanner::default();
        assert_eq!(
            q.feed(b"static let authPhrases = [\"run /login\", \"not logged in\"]"),
            None
        );
    }

    #[test]
    fn rate_limit_menu_fires_with_cooldown() {
        let t0 = Instant::now();
        let mut r = RateLimitScanner::default();
        assert!(!r.feed(b"ordinary output", t0));
        assert!(r.feed(
            b"What do you want to do?\r\n\x1b[1m1. Stop and wait for limit to reset\x1b[0m",
            t0
        ));
        assert!(
            !r.feed(b"1. Stop and wait for limit to reset", t0 + secs(10)),
            "冷却期内不重复"
        );
        assert!(
            r.feed(b"/rate-limit-options", t0 + secs(31)),
            "冷却期满再命中 = 重试"
        );
    }

    #[test]
    fn quota_recovery_needs_a_fresh_long_enough_streak() {
        let t0 = Instant::now();
        let quota = Health::new(HealthKind::RateLimited, "x");
        let other = Health::new(HealthKind::PromptUndelivered, "x");
        assert!(quota_recovered(
            Some(&quota),
            Some(t0),
            Some(t0 + secs(1)),
            t0 + secs(7)
        ));
        assert!(!quota_recovered(
            Some(&quota),
            Some(t0),
            Some(t0 + secs(1)),
            t0 + secs(6)
        ));
        assert!(
            !quota_recovered(Some(&quota), Some(t0), Some(t0), t0 + secs(60)),
            "streak 早于撞墙不算"
        );
        assert!(!quota_recovered(
            Some(&quota),
            Some(t0),
            None,
            t0 + secs(60)
        ));
        assert!(!quota_recovered(
            Some(&other),
            Some(t0),
            Some(t0 + secs(1)),
            t0 + secs(60)
        ));
    }

    fn since() -> SystemTime {
        SystemTime::UNIX_EPOCH + secs(1_800_000_000)
    }

    fn record(failed: bool, offset_secs: i64) -> Value {
        let ts =
            chrono::DateTime::<chrono::Utc>::from(since()) + chrono::Duration::seconds(offset_secs);
        let mut r = serde_json::json!({
            "sessionId": "owned", "type": "assistant", "isSidechain": false,
            "timestamp": ts.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            "message": {"role": "assistant", "id": "msg_real", "model": "claude-model",
                        "content": [{"type": "text", "text": "run /login is quoted"}]},
        });
        if failed {
            r["isApiErrorMessage"] = Value::Bool(true);
            r["error"] = "authentication_failed".into();
        }
        r
    }

    fn jsonl(records: &[Value]) -> Vec<u8> {
        let mut out = Vec::new();
        for r in records {
            out.extend(serde_json::to_vec(r).unwrap());
            out.push(b'\n');
        }
        out
    }

    fn state(records: &[Value]) -> AuthState {
        auth_state_in(&jsonl(records), "owned", since())
    }

    #[test]
    fn auth_failure_and_recovery_follow_the_newest_owned_record() {
        assert_eq!(state(&[]), AuthState::Unknown);
        assert_eq!(state(&[record(false, 1)]), AuthState::Authenticated);
        assert_eq!(state(&[record(true, 1)]), AuthState::AuthenticationRequired);
        assert_eq!(
            state(&[record(true, 1), record(false, 2)]),
            AuthState::Authenticated
        );
        assert_eq!(
            state(&[record(true, 1), record(false, 2), record(true, 3)]),
            AuthState::AuthenticationRequired
        );
    }

    #[test]
    fn auth_evidence_must_be_current_owned_and_from_the_runner() {
        let mut other = record(true, 1);
        other["sessionId"] = "other".into();
        let mut sidechain = record(true, 1);
        sidechain["isSidechain"] = Value::Bool(true);
        let mut user = record(true, 1);
        user["type"] = "user".into();
        let mut synthetic = record(false, 1);
        synthetic["message"]["model"] = "<synthetic>".into();
        for r in [other, sidechain, user, synthetic, record(true, -10)] {
            assert_eq!(state(std::slice::from_ref(&r)), AuthState::Unknown, "{r}");
        }
        let mut partial = jsonl(&[record(true, 1)]);
        partial.pop();
        assert_eq!(
            auth_state_in(&partial, "owned", since()),
            AuthState::Unknown,
            "半截记录不算"
        );
        assert_eq!(
            auth_state_in(b"broken\n", "owned", since()),
            AuthState::Unknown
        );
    }

    #[test]
    fn auth_state_only_touches_auth_health() {
        let quota = Health::new(HealthKind::UsageLimit, "q");
        assert_eq!(
            AuthState::Unknown.apply(Some(quota.clone())),
            Some(quota.clone())
        );
        assert_eq!(AuthState::Unknown.apply(None), None);
        let auth = AuthState::AuthenticationRequired.apply(None).unwrap();
        assert_eq!(auth.kind, HealthKind::AuthRequired);
        assert_eq!(AuthState::Authenticated.apply(Some(auth)), None);
    }

    #[test]
    fn probe_reads_disk_and_refuses_ambiguity() {
        let root = tempfile::tempdir().unwrap();
        let probe = AuthProbe {
            session_id: "owned".into(),
            projects_dir: root.path().to_path_buf(),
            since: since(),
        };
        assert_eq!(probe.read(), AuthState::Unknown);
        for (name, expect) in [
            ("a", AuthState::AuthenticationRequired),
            ("b", AuthState::Unknown),
        ] {
            let d = root.path().join(name);
            std::fs::create_dir(&d).unwrap();
            std::fs::write(d.join("owned.jsonl"), jsonl(&[record(true, 1)])).unwrap();
            assert_eq!(probe.read(), expect, "两个目录都有同名日志 = 有歧义");
        }
        let bad = AuthProbe {
            session_id: "../owned".into(),
            ..probe
        };
        assert_eq!(bad.read(), AuthState::Unknown);
    }

    #[test]
    fn probe_reads_only_the_bounded_tail() {
        let root = tempfile::tempdir().unwrap();
        let d = root.path().join("p");
        std::fs::create_dir(&d).unwrap();
        let mut data = vec![b' '; AuthProbe::MAX_BYTES as usize + 20];
        data.push(b'\n');
        data.extend(jsonl(&[record(true, 1)]));
        std::fs::write(d.join("owned.jsonl"), data).unwrap();
        let probe = AuthProbe {
            session_id: "owned".into(),
            projects_dir: root.path().to_path_buf(),
            since: since(),
        };
        assert_eq!(probe.read(), AuthState::AuthenticationRequired);
        std::fs::remove_file(d.join("owned.jsonl")).unwrap();
        std::fs::create_dir(d.join("owned.jsonl")).unwrap();
        assert_eq!(probe.read(), AuthState::Unknown, "读不了日志不等于没登录");
    }

    #[test]
    fn projects_dir_resolution() {
        let wd = Path::new("/work");
        assert_eq!(
            AuthProbe::projects_dir_for(None, Some("/home/u"), wd),
            Some(PathBuf::from("/home/u/.claude/projects"))
        );
        assert_eq!(
            AuthProbe::projects_dir_for(Some("cfg"), Some("/home/u"), wd),
            Some(PathBuf::from("/work/cfg/projects"))
        );
        assert_eq!(
            AuthProbe::projects_dir_for(Some("/abs"), None, wd),
            Some(PathBuf::from("/abs/projects"))
        );
        assert_eq!(AuthProbe::projects_dir_for(None, None, wd), None);
    }

    #[test]
    fn launch_verdicts() {
        let d = FIRST_OUTPUT_DEADLINE;
        use LaunchVerdict::*;
        assert_eq!(
            launch_verdict(false, false, false, true, secs(0), d),
            SpawnFailed
        );
        assert_eq!(
            launch_verdict(true, false, true, false, secs(1), d),
            DiedSilently
        );
        assert_eq!(
            launch_verdict(true, false, false, false, secs(1), d),
            Pending,
            "还没生 ≠ 死了"
        );
        assert_eq!(launch_verdict(true, false, false, false, d, d), SpawnFailed);
        assert_eq!(
            launch_verdict(true, true, true, false, secs(24), d),
            Pending
        );
        assert_eq!(launch_verdict(true, true, true, false, d, d), Stalled);
        assert_eq!(
            launch_verdict(true, false, true, true, secs(1), d),
            Alive,
            "见过输出就归退出回调管"
        );
        assert!(Stalled.is_terminal() && DiedSilently.is_terminal() && SpawnFailed.is_terminal());
        assert!(!Pending.is_terminal() && !Alive.is_terminal());
    }

    #[test]
    fn launch_failure_detail() {
        let h = launch_failure(LaunchVerdict::DiedSilently, secs(25), Some("exit 1")).unwrap();
        assert_eq!(h.kind, HealthKind::LaunchFailed);
        assert_eq!(h.launch_verdict, Some(LaunchVerdict::DiedSilently));
        assert!(h.detail.contains("（exit 1）"), "{}", h.detail);
        assert!(
            launch_failure(LaunchVerdict::Stalled, secs(25), None)
                .unwrap()
                .detail
                .contains("25 秒")
        );
        assert_eq!(launch_failure(LaunchVerdict::Alive, secs(25), None), None);
    }
}
```

- [ ] **Step 3: 跑测试确认失败**

Run: `cargo test -p crewd-exec health::`
Expected: 编译失败，``unresolved import `LaunchVerdict` ``（测试里的 `use LaunchVerdict::*;` 先报，编译器停在这一条）。

- [ ] **Step 4: 实现**

放在 `crates/exec/src/health.rs` 的测试模块之前：

```rust
//! 会话健康：进程还活着，但 agent 已经干不了活（Swift `SessionHealth` /
//! `SessionLaunchProbe` 的移植）。只报能可靠拿到的事实，不猜。
//!
//! - 额度：PTY 字节尾窗里的额度短语（每次武装只报一次）。
//! - 限流菜单：尾窗里的 `/rate-limit-options` 菜单 → 调用方自动答「1」（30s 冷却）。
//! - 恢复：额度类 health 置上**之后**开始的一段连续干活满 6s = 已恢复。
//! - 认证：读同一会话的 claude 结构化日志（`projects/*/<id>.jsonl`），不看 PTY 文案。
//! - 拉起自检：25s 内一个字节都没有 / 起来即死 / 根本没起来。
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use crewd_term::{AnsiPlainTextTail, RATE_LIMIT_MENU_PHRASES};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HealthKind {
    /// 未登录 / 登录失效。
    AuthRequired,
    /// 额度到顶 / 余额不足。
    UsageLimit,
    /// 卡在限流菜单（已自动答「等重置」）。
    RateLimited,
    /// 拉起失败（见 `launch_verdict`）。
    LaunchFailed,
    /// 开场任务没送进输入框。
    BriefUndelivered,
    /// 运行中的一条指令没送进输入框。
    PromptUndelivered,
}

impl HealthKind {
    /// 额度类：退出原因「撞额度」与额度重置唤醒都认这组。
    pub fn is_quota(self) -> bool {
        matches!(self, Self::UsageLimit | Self::RateLimited)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Health {
    pub kind: HealthKind,
    /// 人话说明 + 下一步。
    pub detail: String,
    /// 只有拉起自检写这项。
    pub launch_verdict: Option<LaunchVerdict>,
}

impl Health {
    pub fn new(kind: HealthKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
            launch_verdict: None,
        }
    }
}

/// 额度短语（小写子串）。
pub const QUOTA_PHRASES: [&str; 6] = [
    "you've reached your",
    "you\u{2019}ve reached your",
    "usage credit limit reached",
    "out of extra usage",
    "credit balance is too low",
    "credit balance too low",
];

/// 额度扫描器：命中一次后哑掉，直到 [`QuotaScanner::rearm`]。
#[derive(Debug, Default)]
pub struct QuotaScanner {
    tail: AnsiPlainTextTail,
    fired: bool,
}

impl QuotaScanner {
    /// 喂 PTY 原始字节；新命中时返回 `UsageLimit`。
    pub fn feed(&mut self, bytes: &[u8]) -> Option<Health> {
        if self.fired || !self.tail.feed(bytes) {
            return None;
        }
        let hit = QUOTA_PHRASES.iter().any(|p| {
            self.tail
                .contains_lowered_ascii(&AnsiPlainTextTail::lowered_needle(p))
        });
        if !hit {
            return None;
        }
        self.fired = true;
        Some(Health::new(
            HealthKind::UsageLimit,
            "Claude Code 额度受限（usage limit / 余额不足）——等额度窗口重置或检查订阅后再继续。",
        ))
    }

    /// 重新武装（额度恢复后），下个限额窗再撞墙能再报。尾窗一并清掉，免得旧短语立刻再命中。
    pub fn rearm(&mut self) {
        self.fired = false;
        self.tail.clear();
    }
}

/// 限流菜单的自动应答冷却。
pub const RATE_LIMIT_COOLDOWN: Duration = Duration::from_secs(30);

/// 限流菜单检测：命中且不在冷却期 → `true`（调用方发「1」、隔 250ms 发回车）。
/// 命中后清空尾窗：按键没生效、菜单还在重绘的话，冷却期满会再次命中 = 内建重试。
#[derive(Debug)]
pub struct RateLimitScanner {
    tail: AnsiPlainTextTail,
    cooldown: Duration,
    last_fired: Option<Instant>,
}

impl Default for RateLimitScanner {
    fn default() -> Self {
        Self::with_cooldown(RATE_LIMIT_COOLDOWN)
    }
}

impl RateLimitScanner {
    pub fn with_cooldown(cooldown: Duration) -> Self {
        Self {
            tail: AnsiPlainTextTail::new(),
            cooldown,
            last_fired: None,
        }
    }

    pub fn feed(&mut self, bytes: &[u8], now: Instant) -> bool {
        if !self.tail.feed(bytes) {
            return false;
        }
        let hit = RATE_LIMIT_MENU_PHRASES.iter().any(|p| {
            self.tail
                .contains_lowered_ascii(&AnsiPlainTextTail::lowered_needle(p))
        });
        if !hit {
            return false;
        }
        if self
            .last_fired
            .is_some_and(|last| now.saturating_duration_since(last) < self.cooldown)
        {
            return false;
        }
        self.last_fired = Some(now);
        self.tail.clear();
        true
    }
}

/// 额度类 health 之后连续干活多久算恢复。
pub const QUOTA_WORK_STREAK: Duration = Duration::from_secs(6);

/// 额度类 health 是否已恢复：干活 streak 起点晚于置上时刻，且已持续 `QUOTA_WORK_STREAK`。
pub fn quota_recovered(
    health: Option<&Health>,
    quota_at: Option<Instant>,
    working_since: Option<Instant>,
    now: Instant,
) -> bool {
    let (Some(h), Some(at), Some(since)) = (health, quota_at, working_since) else {
        return false;
    };
    h.kind.is_quota() && since > at && now.saturating_duration_since(since) >= QUOTA_WORK_STREAK
}

/// 结构化日志给出的认证证据。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthState {
    Unknown,
    Authenticated,
    AuthenticationRequired,
}

impl AuthState {
    /// 把认证证据叠到当前 health 上：失败 → `AuthRequired`；其余只清掉旧的 `AuthRequired`。
    pub fn apply(self, health: Option<Health>) -> Option<Health> {
        match self {
            Self::AuthenticationRequired => Some(Health::new(
                HealthKind::AuthRequired,
                "Claude Code 在本 session 报告了认证失败。请检查该 session 的登录或凭据配置。",
            )),
            Self::Authenticated | Self::Unknown => {
                health.filter(|h| h.kind != HealthKind::AuthRequired)
            }
        }
    }
}

/// 认证探测多久跑一次。
pub const AUTH_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// 读 `projects_dir/*/<session_id>.jsonl` 末尾一段，看这次启动以来最后一条
/// assistant 记录是认证失败还是正常回答。
#[derive(Clone, Debug)]
pub struct AuthProbe {
    pub session_id: String,
    pub projects_dir: PathBuf,
    /// 只认这个时刻之后的记录（本次启动）。
    pub since: SystemTime,
}

impl AuthProbe {
    /// 最多读日志末尾这么多字节。
    pub const MAX_BYTES: u64 = 256 * 1024;

    /// claude 的 projects 目录：`CLAUDE_CONFIG_DIR`（相对路径按 `workdir` 解析）或 `~/.claude`。
    pub fn projects_dir_for(
        config_dir: Option<&str>,
        home: Option<&str>,
        workdir: &Path,
    ) -> Option<PathBuf> {
        let base = match config_dir.filter(|d| !d.is_empty()) {
            Some(d) => workdir.join(d),
            None => Path::new(home.filter(|h| !h.is_empty())?).join(".claude"),
        };
        Some(base.join("projects"))
    }

    /// 读不到、对不上、有歧义一律 `Unknown`——读不到日志不等于没登录。
    pub fn read(&self) -> AuthState {
        let id = &self.session_id;
        if id.is_empty() || id.contains(['/', '\\', '\0']) {
            return AuthState::Unknown;
        }
        let Ok(entries) = std::fs::read_dir(&self.projects_dir) else {
            return AuthState::Unknown;
        };
        let candidates: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
            .map(|e| e.path().join(format!("{id}.jsonl")))
            .filter(|p| p.exists())
            .collect();
        let [file] = candidates.as_slice() else {
            return AuthState::Unknown;
        };
        match read_tail(file, Self::MAX_BYTES) {
            Some(data) => auth_state_in(&data, id, self.since),
            None => AuthState::Unknown,
        }
    }
}

/// 读文件末尾 `max` 字节；从中间开始读时丢掉第一条（可能是半截）记录。
fn read_tail(path: &Path, max: u64) -> Option<Vec<u8>> {
    let mut f = std::fs::File::open(path).ok()?;
    let size = f.metadata().ok()?.len();
    let offset = size.saturating_sub(max);
    f.seek(SeekFrom::Start(offset)).ok()?;
    let mut data = Vec::new();
    f.take(max).read_to_end(&mut data).ok()?;
    if offset > 0 {
        let newline = data.iter().position(|&b| b == b'\n')?;
        data.drain(..=newline);
    }
    Some(data)
}

/// 在一段 JSONL 里找本会话、本次启动以来最新的那条 assistant 记录并判定。
/// 最后一段没有换行结尾的（可能写到一半）不算。
pub fn auth_state_in(data: &[u8], session_id: &str, since: SystemTime) -> AuthState {
    let mut state = AuthState::Unknown;
    let mut newest = chrono::DateTime::<chrono::Utc>::from(since);
    let mut lines: Vec<&[u8]> = data.split(|&b| b == b'\n').collect();
    lines.pop();
    for line in lines {
        let Ok(obj) = serde_json::from_slice::<Value>(line) else {
            continue;
        };
        let owned = obj["sessionId"] == session_id
            && obj["type"] == "assistant"
            && obj["isSidechain"] != Value::Bool(true);
        let Some(ts) = obj["timestamp"]
            .as_str()
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
        else {
            continue;
        };
        let ts = ts.with_timezone(&chrono::Utc);
        if !owned || ts < newest {
            continue;
        }
        newest = ts;
        let api_error = obj["isApiErrorMessage"] == Value::Bool(true);
        let msg = &obj["message"];
        state = if api_error && obj["error"] == "authentication_failed" {
            AuthState::AuthenticationRequired
        } else if !api_error
            && obj.get("error").is_none()
            && msg["role"] == "assistant"
            && msg["model"]
                .as_str()
                .is_some_and(|m| !m.is_empty() && !m.starts_with('<'))
            && msg["id"].as_str().is_some_and(|i| i.starts_with("msg_"))
            && msg["content"].is_array()
        {
            AuthState::Authenticated
        } else {
            AuthState::Unknown
        };
    }
    state
}

/// 拉起自检的观察窗与轮询间隔。
pub const FIRST_OUTPUT_DEADLINE: Duration = Duration::from_secs(25);
pub const LAUNCH_POLL_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchVerdict {
    /// 观察窗内、没有坏消息——继续看。
    Pending,
    /// 根本没起来。
    SpawnFailed,
    /// 起过、没吐一个字就没了。
    DiedSilently,
    /// 活着，但过了期限一个字节都没吐。
    Stalled,
    /// 见过输出——正常，自检收工。
    Alive,
}

impl LaunchVerdict {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::SpawnFailed | Self::DiedSilently | Self::Stalled)
    }
}

/// 拉起自检判定（纯函数）。见过输出优先于一切（除了没 spawn 起来）。
pub fn launch_verdict(
    spawned: bool,
    process_alive: bool,
    ever_alive: bool,
    saw_output: bool,
    elapsed: Duration,
    deadline: Duration,
) -> LaunchVerdict {
    if !spawned {
        return LaunchVerdict::SpawnFailed;
    }
    if saw_output {
        return LaunchVerdict::Alive;
    }
    if process_alive {
        return if elapsed >= deadline {
            LaunchVerdict::Stalled
        } else {
            LaunchVerdict::Pending
        };
    }
    if ever_alive {
        return LaunchVerdict::DiedSilently;
    }
    if elapsed >= deadline {
        LaunchVerdict::SpawnFailed
    } else {
        LaunchVerdict::Pending
    }
}

/// 失败裁决 → health（`Alive`/`Pending` 返回 `None`）。`underlying` 是附带的原因。
pub fn launch_failure(
    verdict: LaunchVerdict,
    deadline: Duration,
    underlying: Option<&str>,
) -> Option<Health> {
    let tail = underlying.map(|u| format!("（{u}）")).unwrap_or_default();
    let detail = match verdict {
        LaunchVerdict::SpawnFailed => format!(
            "Claude Code 子进程没能启动{tail}——这个 session 从头到尾没跑起来，活等于没派出去，请改派或排查后重起。"
        ),
        LaunchVerdict::DiedSilently => format!(
            "Claude Code 子进程启动后立刻退出{tail}——常见于额度耗尽 / 未登录 / 工作目录不可用，请核实后重起。"
        ),
        LaunchVerdict::Stalled => format!(
            "Claude Code 起来了但 {} 秒内一个字都没输出{tail}——半死状态（进程在、干不了活），请改派或重起。",
            deadline.as_secs()
        ),
        LaunchVerdict::Alive | LaunchVerdict::Pending => return None,
    };
    Some(Health {
        kind: HealthKind::LaunchFailed,
        detail,
        launch_verdict: Some(verdict),
    })
}
```

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -p crewd-exec`
Expected: `49 passed`。

- [ ] **Step 6: 全量检查并提交**

Run: `./scripts/check.sh` → 0。

```bash
git add Cargo.lock crates/exec/Cargo.toml crates/exec/src/lib.rs crates/exec/src/health.rs
git commit -m "crewd-exec: health scanners, transcript auth probe and launch verdicts

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 6: 会话核心（`session::Core`，sans-IO）

一个 claude 会话的全部判断，不碰 IO：`on_output(bytes, now)` / `on_tick(now)` / `send` / `raw` / `resize` / `on_auth` / `stop` / `on_exit`，吐出按顺序执行的 `Action`（`Write` / `Resize` / `Event`）。驱动层（Task 10）只负责搬字节、按 `next_wakeup()` 定时。所以 Swift `AgentSessionCore` 里散在定时器、回调、`Task.sleep` 里的规矩，在这里全能用合成时间、合成画面单测：

- 每块输出：喂 `Screen` → **先把终端回复写回子进程**（claude 每次启动都发 DA1 / XTVERSION / OSC 11，可能正卡着等）→ 置「干活」→ 额度扫描 → 限流菜单（写 `1`，250ms 后回车，翻 `RateLimited`）→ 续跑被拒检查 → 推进投递。
- 每拍：发到点的排队按键 → 干活 / 空闲边沿（距最后输出 1s）→ 额度恢复（6s streak）→ 拉起自检（25s 没输出 = `Stalled`）→ 续跑被拒检查 → 待决策跟踪（**只给屏幕**，裁定 3）→ 推进投递。
- 投递：一次一笔、排队；`WriteBody` 重投先 Ctrl-U；正文先过 `sanitize_text`（裁定 7）；`Submit` = 空格、100ms 退格、再 100ms 回车，三拍没发完不回读（裁定 6）；非终局报告翻 health 继续投，终局收掉、成功时清掉同类 health、取下一笔。
- 退出：没见过输出且不是我们停的 = `DiedSilently`；手上和排队的正文报 `Abandoned`（运行中的指令另留一条 `PromptUndelivered`，我们停的不留）；清待决策、落空闲边沿；`hit_limit` = 退出前 600s 内撞过额度。

**Files:**
- Create: `crewd/crates/exec/src/session.rs`
- Modify: `crewd/crates/exec/src/lib.rs`

**Interfaces:**
- Consumes: Task 4 `delivery::{Delivery, Observation, Step, Subject, Timing}`；Task 5 `health::{AuthState, FIRST_OUTPUT_DEADLINE, Health, HealthKind, LaunchVerdict, QuotaScanner, RateLimitScanner, launch_failure, launch_verdict, quota_recovered}`；M2a `crewd_term::{AnsiPlainTextTail, DecisionEvent, PendingDecision, PendingDecisionTracker, Screen, ScreenConfig, Snapshot, blocking_dialog, body_tail_visible, input_row}`。
- Produces（`crewd_exec::session`）:
  - `BRIEF_DELIVERY_ID: u64 = 0`、`NO_CONVERSATION_MARKER: &str`
  - `#[derive(Clone, Copy)] pub struct CoreConfig { pub delivery: Timing, pub working_quiet, pub submit_key_gap, pub rate_limit_answer_gap, pub hit_limit_window, pub resume_refusal_window, pub launch_deadline: Duration }`（`Default` = 1s / 100ms / 250ms / 600s / 5s / 25s）
  - `pub struct PendingText { pub id: u64, pub subject: Subject, pub text: String }`
  - `pub enum DeliveryOutcome { Delivered, Blocked(String), NotReady(String), Undelivered(String), Unsubmitted(String), Abandoned(String) }`（`is_final()`）
  - `pub enum ExitStatusKind { Code(i32), Signal(i32) }`、`pub struct ExitInfo { pub status: ExitStatusKind, pub stopped_by_user: bool, pub hit_limit: bool }`
  - `pub enum SessionEvent { Spawned { pid: u32, agent_session_id: String, resumed: bool }, Working(bool), Decision(DecisionEvent), Delivery { id: u64, subject: Subject, outcome: DeliveryOutcome }, Health(Option<Health>), ResumeRefused { id: String, said: String }, Exited(ExitInfo) }`（`Spawned` 只由驱动层发）
  - `pub enum Action { Write(Vec<u8>), Resize { cols: u16, lines: u16 }, Event(SessionEvent) }`
  - `pub enum DerivedState { LaunchFailed, Exited, RateLimited, Error, AwaitingDecision, Working, Idle }`
  - `impl Core`：`fn new(CoreConfig, ScreenConfig, now: Instant, pending: Vec<PendingText>, resumed_id: Option<String>) -> Core`、`take_actions(&mut self) -> Vec<Action>`、`next_wakeup(&self) -> Option<Instant>`、`is_running`、`health() -> Option<&Health>`、`pending_decision() -> Option<&PendingDecision>`、`is_working`、`take_pending(&mut self) -> Vec<PendingText>`、`state() -> DerivedState`、`screen_text(usize) -> String`、`snapshot() -> Snapshot`、`on_output(&[u8], Instant)`、`on_tick(Instant)`、`send(u64, &str, Instant)`、`raw(&[u8])`、`resize(u16, u16)`、`on_auth(AuthState, Instant)`、`clear_quota_health(Instant)`、`stop(Instant)`、`on_exit(ExitStatusKind, Instant)`
  - `fn sanitize_text(&str) -> String`、`fn preview(&str) -> String`、`fn resume_rejection(screen_text: &str, resumed_id: &str) -> Option<String>`

- [ ] **Step 1: 写失败的测试**

`crates/exec/src/lib.rs` 整个换成：

```rust
//! crewd 的执行器：起 agent 进程（PTY）、读它的屏幕、投递正文、认健康状态、收尸与恢复。
//! 其余部分只经 `driver::SessionHandle` 这一个窄接口与它交互（spec §13 Q12）。
pub mod delivery;
pub mod env;
pub mod health;
pub mod launch;
pub mod pty;
pub mod session;
```

`crates/exec/src/session.rs`：先只放测试模块。合成画面 `input_box(..)` 每次整屏重画（`ESC[2J ESC[H`），`drive_one` 把「就绪 → 稳住 0.3s 写正文 → 回显 → 三拍提交 → 清空」走一遍；最后两条拿 M2a 录下来的真字节（`../term/tests/fixtures/`）验信任框上绝不投、就绪屏上照投。Review Focus 1 / 3 的测试在这里：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    struct T {
        core: Core,
        t0: Instant,
    }

    impl T {
        fn new(pending: Vec<PendingText>) -> Self {
            Self::with(pending, None)
        }

        fn with(pending: Vec<PendingText>, resumed: Option<&str>) -> Self {
            let t0 = Instant::now();
            Self {
                core: Core::new(
                    CoreConfig::default(),
                    ScreenConfig::default(),
                    t0,
                    pending,
                    resumed.map(str::to_string),
                ),
                t0,
            }
        }

        fn at(&self, secs: f64) -> Instant {
            self.t0 + Duration::from_secs_f64(secs)
        }

        fn output(&mut self, bytes: &[u8], secs: f64) -> Vec<Action> {
            let now = self.at(secs);
            self.core.on_output(bytes, now);
            self.core.take_actions()
        }

        fn tick(&mut self, secs: f64) -> Vec<Action> {
            let now = self.at(secs);
            self.core.on_tick(now);
            self.core.take_actions()
        }
    }

    fn brief(text: &str) -> Vec<PendingText> {
        vec![PendingText {
            id: BRIEF_DELIVERY_ID,
            subject: Subject::Brief,
            text: text.to_string(),
        }]
    }

    /// 合成的 claude 输入框画面（整屏重画）。
    fn input_box(input: &str) -> Vec<u8> {
        format!(
            "\x1b[2J\x1b[HClaude Code v9.9.9\r\n\r\n╭──────────────────────╮\r\n│ ❯ {input} \r\n╰──────────────────────╯\r\n  ? for shortcuts"
        )
        .into_bytes()
    }

    fn trust_dialog() -> Vec<u8> {
        "\x1b[2J\x1b[HQuick safety check: Is this a project you created or one you trust?\r\n\r\n❯  No, exit\r\n   Yes, I trust this folder\r\n\r\nEnter to confirm · Esc to cancel"
            .as_bytes()
            .to_vec()
    }

    fn writes(actions: &[Action]) -> Vec<Vec<u8>> {
        actions
            .iter()
            .filter_map(|a| match a {
                Action::Write(b) => Some(b.clone()),
                _ => None,
            })
            .collect()
    }

    fn events(actions: &[Action]) -> Vec<SessionEvent> {
        actions
            .iter()
            .filter_map(|a| match a {
                Action::Event(e) => Some(e.clone()),
                _ => None,
            })
            .collect()
    }

    fn delivered(actions: &[Action], id: u64) -> bool {
        events(actions).iter().any(|e| {
            matches!(e, SessionEvent::Delivery { id: i, outcome: DeliveryOutcome::Delivered, .. } if *i == id)
        })
    }

    /// 从「输入框就绪」开始把一笔投完：画框 → 稳住 0.3s 写正文 → 回显 → 三拍提交 → 清空。
    fn drive_one(t: &mut T, text: &str, start: f64) -> Vec<Action> {
        let mut all = t.output(&input_box(""), start);
        all.extend(t.tick(start + 0.3));
        assert!(
            writes(&all).iter().any(|w| w.ends_with(text.as_bytes())),
            "输入框稳住后应写正文：{all:?}"
        );
        all.extend(t.output(&input_box(text), start + 0.9));
        assert_eq!(writes(&all).last().unwrap(), b" ", "落地后先发空格");
        all.extend(t.tick(start + 1.0));
        assert_eq!(writes(&all).last().unwrap(), &[0x7f]);
        all.extend(t.tick(start + 1.1));
        assert_eq!(writes(&all).last().unwrap(), b"\r");
        all.extend(t.output(&input_box(""), start + 1.2));
        all
    }

    #[test]
    fn terminal_replies_are_written_back_first() {
        let mut t = T::new(vec![]);
        let actions = t.output(b"\x1b[c", 0.0);
        assert_eq!(actions[0], Action::Write(b"\x1b[?6c".to_vec()));
    }

    #[test]
    fn brief_is_written_only_once_the_box_exists_then_submitted() {
        let mut t = T::new(brief("第一条任务"));
        let early = t.output(b"\x1b[2J\x1b[HClaude Code v9.9.9 starting...", 0.0);
        assert!(
            writes(&early).is_empty(),
            "没有输入框一个字节都不写：{early:?}"
        );
        let all = drive_one(&mut t, "第一条任务", 1.0);
        assert!(delivered(&all, BRIEF_DELIVERY_ID), "{all:?}");
    }

    /// 提交三拍发完之前，输入行的变化不算「已提交」：粘贴占位符会把那个空格算成多一行
    /// （`+2 lines` → `+3 lines`），退格之后又变回去——那一帧不能当成回车被接受。
    #[test]
    fn row_change_mid_submit_sequence_is_not_success() {
        let mut t = T::new(brief("一\n二"));
        t.output(&input_box(""), 0.0);
        t.tick(0.3);
        let submit = t.output(&input_box("[Pasted text #1 +2 lines]"), 0.9);
        assert_eq!(writes(&submit).last().unwrap(), b" ");
        let mid = t.output(&input_box("[Pasted text #1 +3 lines]"), 0.95);
        assert!(
            !delivered(&mid, BRIEF_DELIVERY_ID),
            "空格那一帧不算：{mid:?}"
        );
        assert_eq!(writes(&t.tick(1.0)), vec![vec![0x7f]]);
        t.output(&input_box("[Pasted text #1 +2 lines]"), 1.05);
        assert_eq!(writes(&t.tick(1.1)), vec![b"\r".to_vec()]);
        let done = t.output(&input_box(""), 1.2);
        assert!(delivered(&done, BRIEF_DELIVERY_ID), "{done:?}");
    }

    #[test]
    fn carriage_returns_inside_text_never_submit_early() {
        assert_eq!(sanitize_text("a\r\nb\rc\nd"), "a\nb\nc\nd");
        let mut t = T::new(vec![]);
        t.core.send(7, "一\r二", t.at(0.0));
        let mut actions = t.output(&input_box(""), 0.1);
        actions.extend(t.tick(0.4));
        assert!(
            writes(&actions).contains(&"一\n二".as_bytes().to_vec()),
            "{actions:?}"
        );
    }

    /// Review Focus 1：白板里贴进来的终端输出带 ESC / Ctrl-C——绝不能以按键的形式到达 claude。
    #[test]
    fn control_characters_in_text_are_made_inert() {
        assert_eq!(
            sanitize_text("red \x1b[31mX\x1b[0m\tok\x03\x00\x7f\u{9b}"),
            "red \u{241b}[31mX\u{241b}[0m\tok\u{2403}\u{2400}\u{2421}\u{fffd}"
        );
        let mut t = T::new(vec![]);
        t.core.send(3, "贴进来的 \x1b[2J 和 \x03", t.at(0.0));
        let mut actions = t.output(&input_box(""), 0.1);
        actions.extend(t.tick(0.4));
        let body = writes(&actions).concat();
        assert!(!body.is_empty(), "应当写了正文：{actions:?}");
        assert!(
            !body.iter().any(|&b| b == 0x1b || b == 0x03),
            "正文里不许出现 ESC / Ctrl-C：{body:?}"
        );
    }

    #[test]
    fn retry_clears_the_input_row_with_ctrl_u() {
        let mut t = T::new(brief("x"));
        t.output(&input_box(""), 0.0);
        assert_eq!(writes(&t.tick(0.3)), vec![b"x".to_vec()]);
        let retry = t.tick(2.8);
        assert_eq!(writes(&retry), vec![b"\x15x".to_vec()]);
    }

    #[test]
    fn steers_queue_behind_the_one_in_flight() {
        let mut t = T::new(vec![]);
        t.core.send(1, "甲", t.at(0.0));
        t.core.send(2, "乙", t.at(0.0));
        let first = drive_one(&mut t, "甲", 0.0);
        assert!(delivered(&first, 1));
        assert!(
            !writes(&first).iter().any(|w| w.ends_with("乙".as_bytes())),
            "第二笔要等第一笔收工"
        );
        let second = drive_one(&mut t, "乙", 1.5);
        assert!(delivered(&second, 2), "{second:?}");
    }

    #[test]
    fn dialog_blocks_the_brief_then_it_is_delivered_after_the_answer() {
        let mut t = T::new(brief("开场"));
        let mut all = t.output(&trust_dialog(), 0.0);
        for s in 1..=4 {
            all.extend(t.tick(s as f64));
        }
        assert!(writes(&all).is_empty(), "对话框在场一个字节都不投：{all:?}");
        let ev = events(&all);
        assert!(ev.iter().any(
            |e| matches!(e, SessionEvent::Decision(DecisionEvent::Appeared(d)) if !d.numbered)
        ));
        assert!(ev.iter().any(|e| matches!(
            e,
            SessionEvent::Delivery {
                outcome: DeliveryOutcome::Blocked(_),
                ..
            }
        )));
        assert_eq!(
            t.core.health().map(|h| h.kind),
            Some(HealthKind::BriefUndelivered)
        );
        assert_eq!(t.core.state(), DerivedState::Error);

        // 有人代答 → 输入框出现 → 自己补投；health 跟着清掉。
        let after = drive_one(&mut t, "开场", 5.0);
        assert!(delivered(&after, BRIEF_DELIVERY_ID), "{after:?}");
        assert!(events(&after).contains(&SessionEvent::Health(None)));
        assert!(events(&after).contains(&SessionEvent::Decision(DecisionEvent::Cleared)));
    }

    #[test]
    fn working_and_idle_edges() {
        let mut t = T::new(vec![]);
        let a = t.output(b"thinking", 0.0);
        assert_eq!(events(&a), vec![SessionEvent::Working(true)]);
        assert!(events(&t.tick(0.6)).is_empty());
        assert_eq!(events(&t.tick(1.2)), vec![SessionEvent::Working(false)]);
        assert_eq!(t.core.state(), DerivedState::Idle);
    }

    #[test]
    fn rate_limit_menu_is_answered_and_recovers_after_sustained_work() {
        let mut t = T::new(vec![]);
        let a = t.output(b"What do you want to do?\r\n\xe2\x9d\xaf 1. Stop and wait for limit to reset\r\n  2. Upgrade", 0.0);
        assert_eq!(writes(&a), vec![b"1".to_vec()]);
        assert_eq!(
            t.core.health().map(|h| h.kind),
            Some(HealthKind::RateLimited)
        );
        assert_eq!(t.core.state(), DerivedState::RateLimited);
        assert!(writes(&t.tick(0.1)).is_empty());
        assert_eq!(writes(&t.tick(0.25)), vec![b"\r".to_vec()]);
        assert_eq!(t.core.pending_decision(), None, "限流菜单不报给人");
        // 安静一阵，再连续干活 6s 以上 = 已恢复。
        t.tick(2.0);
        let mut recovered = Vec::new();
        for i in 0..=14 {
            let s = 3.0 + i as f64 * 0.5;
            recovered.extend(t.output(b".", s));
            recovered.extend(t.tick(s));
        }
        assert!(
            events(&recovered).contains(&SessionEvent::Health(None)),
            "{recovered:?}"
        );
    }

    #[test]
    fn usage_limit_marks_the_exit_as_hit_limit() {
        let mut t = T::new(vec![]);
        t.output(b"You've reached your usage limit", 0.0);
        assert_eq!(
            t.core.health().map(|h| h.kind),
            Some(HealthKind::UsageLimit)
        );
        t.core.on_exit(ExitStatusKind::Code(1), t.at(30.0));
        let ev = events(&t.core.take_actions());
        assert!(
            ev.contains(&SessionEvent::Exited(ExitInfo {
                status: ExitStatusKind::Code(1),
                stopped_by_user: false,
                hit_limit: true,
            })),
            "{ev:?}"
        );
    }

    #[test]
    fn silent_child_is_reported_stalled_at_the_deadline() {
        let mut t = T::new(vec![]);
        assert!(
            t.tick(24.0)
                .iter()
                .all(|a| !matches!(a, Action::Event(SessionEvent::Health(_))))
        );
        t.tick(25.0);
        let h = t.core.health().unwrap();
        assert_eq!(
            (h.kind, h.launch_verdict),
            (HealthKind::LaunchFailed, Some(LaunchVerdict::Stalled))
        );
        assert_eq!(t.core.state(), DerivedState::LaunchFailed);
    }

    #[test]
    fn dying_before_any_output_is_a_launch_failure_unless_we_stopped_it() {
        let mut t = T::new(vec![]);
        t.core.on_exit(ExitStatusKind::Code(1), t.at(0.5));
        let h = t.core.health().unwrap();
        assert_eq!(h.launch_verdict, Some(LaunchVerdict::DiedSilently));
        assert!(h.detail.contains("exit 1"), "{}", h.detail);

        let mut t = T::new(vec![]);
        t.core.stop(t.at(0.5));
        t.core.on_exit(ExitStatusKind::Signal(15), t.at(0.6));
        assert_eq!(t.core.health(), None, "我们自己停的不算拉起失败");
        assert!(
            events(&t.core.take_actions()).contains(&SessionEvent::Exited(ExitInfo {
                status: ExitStatusKind::Signal(15),
                stopped_by_user: true,
                hit_limit: false,
            }))
        );
    }

    #[test]
    fn exit_reports_lost_steers_but_stop_does_not() {
        let mut t = T::new(vec![]);
        t.output(b"busy", 0.0);
        t.core.send(1, "第一条指令", t.at(0.1));
        t.core.send(2, "第二条", t.at(0.1));
        t.core.on_exit(ExitStatusKind::Code(0), t.at(1.0));
        let ev = events(&t.core.take_actions());
        assert_eq!(
            ev.iter()
                .filter(|e| matches!(
                    e,
                    SessionEvent::Delivery {
                        outcome: DeliveryOutcome::Abandoned(_),
                        ..
                    }
                ))
                .count(),
            2
        );
        let h = t.core.health().unwrap();
        assert_eq!(h.kind, HealthKind::PromptUndelivered);
        assert!(
            h.detail.contains("2 条") && h.detail.contains("第一条指令"),
            "{}",
            h.detail
        );

        let mut t = T::new(vec![]);
        t.output(b"busy", 0.0);
        t.core.send(1, "x", t.at(0.1));
        t.core.stop(t.at(0.2));
        assert_eq!(t.core.health(), None);
    }

    #[test]
    fn sending_to_an_exited_session_is_reported_not_dropped() {
        let mut t = T::new(vec![]);
        t.output(b"x", 0.0);
        t.core.on_exit(ExitStatusKind::Code(0), t.at(1.0));
        t.core.take_actions();
        t.core.send(5, "迟到的指令", t.at(2.0));
        let ev = events(&t.core.take_actions());
        assert!(ev.iter().any(|e| matches!(
            e,
            SessionEvent::Delivery {
                id: 5,
                outcome: DeliveryOutcome::Abandoned(_),
                ..
            }
        )));
        assert_eq!(
            t.core.health().map(|h| h.kind),
            Some(HealthKind::PromptUndelivered)
        );
    }

    #[test]
    fn resume_refusal_is_recognised_only_for_our_id_and_only_early() {
        let said = format!("{NO_CONVERSATION_MARKER}: abc-123");
        let mut t = T::with(vec![], Some("abc-123"));
        let ev = events(&t.output(said.as_bytes(), 1.0));
        assert!(
            ev.contains(&SessionEvent::ResumeRefused {
                id: "abc-123".into(),
                said: said.clone()
            }),
            "{ev:?}"
        );

        let mut t = T::with(vec![], Some("abc-123"));
        let ev = events(&t.output(format!("{NO_CONVERSATION_MARKER}: other").as_bytes(), 1.0));
        assert!(
            !ev.iter()
                .any(|e| matches!(e, SessionEvent::ResumeRefused { .. }))
        );

        let mut t = T::with(vec![], Some("abc-123"));
        t.tick(5.1);
        let ev = events(&t.output(said.as_bytes(), 5.2));
        assert!(
            !ev.iter()
                .any(|e| matches!(e, SessionEvent::ResumeRefused { .. })),
            "窗口外不认"
        );

        let mut t = T::new(vec![]);
        let ev = events(&t.output(said.as_bytes(), 1.0));
        assert!(
            !ev.iter()
                .any(|e| matches!(e, SessionEvent::ResumeRefused { .. })),
            "没续跑就不看"
        );
    }

    /// Review Focus 3：attach 的客户端把终端改窄了，那行原话在屏上折成两行——照样认出。
    #[test]
    fn refusal_is_recognised_even_when_wrapped_on_a_narrow_screen() {
        let id = "0b3c5d7e-1111-4222-8333-944455556666";
        let t0 = Instant::now();
        let mut core = Core::new(
            CoreConfig::default(),
            ScreenConfig {
                cols: 40,
                ..ScreenConfig::default()
            },
            t0,
            vec![],
            Some(id.to_string()),
        );
        core.on_output(format!("{NO_CONVERSATION_MARKER}: {id}\r\n").as_bytes(), t0);
        assert!(
            resume_rejection(&core.screen_text(25), id).is_none(),
            "前提：屏上确实折成了两行"
        );
        let ev = events(&core.take_actions());
        assert!(
            ev.iter()
                .any(|e| matches!(e, SessionEvent::ResumeRefused { id: i, .. } if i == id)),
            "{ev:?}"
        );
    }

    /// spec §17 的裁定：只看屏幕。菜单的字节里每行只有 LF（字节尾窗认得出），答完后屏幕
    /// 已经清掉——待决策必须随屏幕清除，不能被尾窗里的旧菜单拖住。
    #[test]
    fn answered_menu_clears_with_the_screen_not_the_byte_tail() {
        let mut t = T::new(vec![]);
        let menu =
            "\x1b[10;1HDo you want to proceed?\n\x1b[11;1H\u{276f} 1. Yes\n\x1b[12;1H  2. No\n";
        t.output(menu.as_bytes(), 0.0);
        t.tick(0.6);
        let shown = t.tick(3.7);
        assert!(
            events(&shown)
                .iter()
                .any(|e| matches!(e, SessionEvent::Decision(DecisionEvent::Appeared(_))))
        );
        t.output("\x1b[2J\x1b[H\u{23fa} ok\n".as_bytes(), 4.0);
        let cleared = t.tick(4.6);
        assert!(
            events(&cleared).contains(&SessionEvent::Decision(DecisionEvent::Cleared)),
            "{cleared:?}"
        );
    }

    #[test]
    fn next_wakeup_covers_scheduled_keys() {
        let mut t = T::new(vec![]);
        t.output(b"/rate-limit-options", 0.0);
        assert_eq!(t.core.next_wakeup(), Some(t.at(0.25)));
    }

    #[test]
    fn resize_is_forwarded_once_and_degenerate_sizes_are_ignored() {
        let mut t = T::new(vec![]);
        t.core.resize(120, 40);
        assert_eq!(
            t.core.take_actions(),
            vec![Action::Resize {
                cols: 120,
                lines: 40
            }]
        );
        t.core.resize(1, 40);
        assert!(t.core.take_actions().is_empty());
    }

    #[test]
    fn preview_is_short_and_single_line() {
        assert_eq!(preview("a\nb"), "a b");
        assert_eq!(preview(&"字".repeat(50)).chars().count(), 41);
    }

    fn fixture(name: &str) -> Vec<u8> {
        let path = format!(
            "{}/../term/tests/fixtures/{name}",
            env!("CARGO_MANIFEST_DIR")
        );
        std::fs::read(&path).unwrap_or_else(|e| panic!("缺夹具 {path}: {e}"))
    }

    /// 真 claude 录下来的字节：未信任目录的首屏是信任框 → 不投、报 Blocked 与待决策。
    #[test]
    fn recorded_trust_screen_blocks_the_brief() {
        let mut t = T::new(brief("开场任务"));
        let mut all = Vec::new();
        for (k, chunk) in fixture("tui-claude-trust.bin").chunks(64).enumerate() {
            all.extend(t.output(chunk, k as f64 * 0.01));
        }
        for s in 1..=5 {
            all.extend(t.tick(s as f64));
        }
        assert!(
            !writes(&all)
                .iter()
                .any(|w| w.ends_with("开场任务".as_bytes())),
            "信任框上绝不投正文"
        );
        let ev = events(&all);
        assert!(
            ev.iter().any(|e| matches!(
                e,
                SessionEvent::Delivery {
                    outcome: DeliveryOutcome::Blocked(_),
                    ..
                }
            )),
            "{ev:?}"
        );
        assert!(
            ev.iter()
                .any(|e| matches!(e, SessionEvent::Decision(DecisionEvent::Appeared(_))))
        );
    }

    /// 已信任目录：录下来的就绪屏上直接投。
    #[test]
    fn recorded_ready_screen_receives_the_brief() {
        let mut t = T::new(brief("开场任务"));
        let mut all = Vec::new();
        for (k, chunk) in fixture("tui-claude-ready.bin").chunks(64).enumerate() {
            all.extend(t.output(chunk, k as f64 * 0.01));
        }
        all.extend(t.tick(3.0));
        assert!(
            writes(&all)
                .iter()
                .any(|w| w.ends_with("开场任务".as_bytes())),
            "就绪屏上应写正文"
        );
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p crewd-exec session::`
Expected: 编译失败，``cannot find type `Core` in this scope``。

- [ ] **Step 3: 实现**

放在 `crates/exec/src/session.rs` 的测试模块之前：

```rust
//! 一个 claude 会话的全部判断，不碰 IO（sans-IO）：喂 PTY 字节、定时拍、命令、退出，
//! 吐出「往 PTY 写什么」和「发生了什么」。时间一律由调用方传入（同 `crewd_term`）。
//!
//! 驱动层（`crate::driver`）只负责把字节搬进搬出、按 [`Core::next_wakeup`] 定时；
//! 所以这里的每条规矩都能用合成时间、合成画面单测。
//!
//! 待决策只看**屏幕**这一路（不喂跟踪器的字节尾窗）：生产 PTY 行尾是 CRLF，尾窗里
//! 行与行之间夹着空行，多行菜单在那一路永远认不出；就算把 CRLF 折掉，尾窗也不知道
//! 屏幕已经翻过去了，答完的菜单会一直挂在 4K 窗里，把「已清除」拖住。见 spec §17。
use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crewd_term::{
    AnsiPlainTextTail, DecisionEvent, PendingDecision, PendingDecisionTracker, Screen,
    ScreenConfig, Snapshot, blocking_dialog, body_tail_visible, input_row,
};

use crate::delivery::{Delivery, Observation, Step, Subject, Timing};
use crate::health::{
    AuthState, FIRST_OUTPUT_DEADLINE, Health, HealthKind, LaunchVerdict, QuotaScanner,
    RateLimitScanner, launch_failure, launch_verdict, quota_recovered,
};

/// 开场任务的投递编号。运行中的指令由 `SessionHandle` 从 1 起编。
pub const BRIEF_DELIVERY_ID: u64 = 0;

/// claude 拒绝续跑时屏上的原话。
pub const NO_CONVERSATION_MARKER: &str = "No conversation found with session ID";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CoreConfig {
    pub delivery: Timing,
    /// 距最后一次输出多久算「不在干活」。
    pub working_quiet: Duration,
    /// 提交三拍（空格 / 退格 / 回车）之间的间隔。
    pub submit_key_gap: Duration,
    /// 限流菜单自动应答「1」与回车之间的间隔。
    pub rate_limit_answer_gap: Duration,
    /// 退出前多久内撞过额度，退出原因记为「撞额度」。
    pub hit_limit_window: Duration,
    /// 续跑被拒只在启动后这段时间里认。
    pub resume_refusal_window: Duration,
    /// 拉起自检的期限。
    pub launch_deadline: Duration,
}

impl Default for CoreConfig {
    fn default() -> Self {
        Self {
            delivery: Timing::default(),
            working_quiet: Duration::from_secs(1),
            submit_key_gap: Duration::from_millis(100),
            rate_limit_answer_gap: Duration::from_millis(250),
            hit_limit_window: Duration::from_secs(600),
            resume_refusal_window: Duration::from_secs(5),
            launch_deadline: FIRST_OUTPUT_DEADLINE,
        }
    }
}

/// 一笔待投的正文。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingText {
    pub id: u64,
    pub subject: Subject,
    pub text: String,
}

/// 一笔投递的结果。`Blocked` / `NotReady` 只是报告，投递仍在继续；其余是终局。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeliveryOutcome {
    Delivered,
    Blocked(String),
    NotReady(String),
    Undelivered(String),
    Unsubmitted(String),
    /// 进程退出 / 被停掉，没投出去。
    Abandoned(String),
}

impl DeliveryOutcome {
    pub fn is_final(&self) -> bool {
        !matches!(self, Self::Blocked(_) | Self::NotReady(_))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExitStatusKind {
    Code(i32),
    Signal(i32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExitInfo {
    pub status: ExitStatusKind,
    /// 是我们（人 / 机长）停的。
    pub stopped_by_user: bool,
    /// 退出前 `hit_limit_window` 内撞过额度。
    pub hit_limit: bool,
}

/// 驱动层往外发的事件（窄接口的状态面）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionEvent {
    /// 进程起来了（驱动层发；续跑被拒重起后会再发一次）。`resumed` = 用了 `--resume`。
    Spawned {
        pid: u32,
        agent_session_id: String,
        resumed: bool,
    },
    /// 干活 / 空闲的边沿。`false` = idle 边沿（「停下即检查」挂在它上面）。
    Working(bool),
    Decision(DecisionEvent),
    Delivery {
        id: u64,
        subject: Subject,
        outcome: DeliveryOutcome,
    },
    /// health 变了（`None` = 恢复正常）。
    Health(Option<Health>),
    /// claude 拒绝 `--resume <id>`，原话 `said`。
    ResumeRefused {
        id: String,
        said: String,
    },
    Exited(ExitInfo),
}

/// 交给驱动层执行的动作，按顺序执行。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Write(Vec<u8>),
    Resize { cols: u16, lines: u16 },
    Event(SessionEvent),
}

/// 推导出的会话状态（坏消息优先）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DerivedState {
    LaunchFailed,
    Exited,
    RateLimited,
    Error,
    AwaitingDecision,
    Working,
    Idle,
}

struct InFlight {
    pending: PendingText,
    machine: Delivery,
}

pub struct Core {
    config: CoreConfig,
    screen: Screen,
    tracker: PendingDecisionTracker,
    quota: QuotaScanner,
    rate_limit: RateLimitScanner,
    started_at: Instant,
    running: bool,
    stopped_by_user: bool,
    last_output_at: Option<Instant>,
    working: bool,
    working_since: Option<Instant>,
    health: Option<Health>,
    quota_at: Option<Instant>,
    launch_settled: bool,
    resume_watch: Option<(String, Instant)>,
    /// 续跑观察窗内的字节尾窗：屏幕被改窄时那行原话会折成两行，尾窗里仍是一行。
    resume_tail: AnsiPlainTextTail,
    pending_decision: Option<PendingDecision>,
    in_flight: Option<InFlight>,
    queue: VecDeque<PendingText>,
    keys: VecDeque<(Instant, Vec<u8>)>,
    actions: Vec<Action>,
}

impl Core {
    /// `pending`：开机就要投的正文（开场任务排第一）。`resumed_id`：本次用 `--resume` 起的会话号。
    pub fn new(
        config: CoreConfig,
        screen: ScreenConfig,
        now: Instant,
        pending: Vec<PendingText>,
        resumed_id: Option<String>,
    ) -> Self {
        let mut core = Self {
            config,
            screen: Screen::new(screen),
            tracker: PendingDecisionTracker::new(),
            quota: QuotaScanner::default(),
            rate_limit: RateLimitScanner::default(),
            started_at: now,
            running: true,
            stopped_by_user: false,
            last_output_at: None,
            working: false,
            working_since: None,
            health: None,
            quota_at: None,
            launch_settled: false,
            resume_watch: resumed_id.map(|id| (id, now + config.resume_refusal_window)),
            resume_tail: AnsiPlainTextTail::new(),
            pending_decision: None,
            in_flight: None,
            queue: pending.into(),
            keys: VecDeque::new(),
            actions: Vec::new(),
        };
        core.start_next(now);
        core
    }

    /// 取走攒下的动作。
    pub fn take_actions(&mut self) -> Vec<Action> {
        std::mem::take(&mut self.actions)
    }

    /// 最早什么时候需要再拍一次（排好的按键、投递的期限）。0.6s 节拍之外的精确唤醒。
    pub fn next_wakeup(&self) -> Option<Instant> {
        let key = self.keys.front().map(|(at, _)| *at);
        let delivery = self
            .in_flight
            .as_ref()
            .and_then(|f| f.machine.next_deadline());
        [key, delivery].into_iter().flatten().min()
    }

    pub fn is_running(&self) -> bool {
        self.running
    }

    pub fn health(&self) -> Option<&Health> {
        self.health.as_ref()
    }

    pub fn pending_decision(&self) -> Option<&PendingDecision> {
        self.pending_decision.as_ref()
    }

    pub fn is_working(&self) -> bool {
        self.working
    }

    /// 还没投出去的正文（在途的排第一），续跑被拒重起时搬到新的 `Core`。
    pub fn take_pending(&mut self) -> Vec<PendingText> {
        let mut out: Vec<PendingText> = self
            .in_flight
            .take()
            .map(|f| f.pending)
            .into_iter()
            .collect();
        out.extend(self.queue.drain(..));
        out
    }

    pub fn state(&self) -> DerivedState {
        if self
            .health
            .as_ref()
            .is_some_and(|h| h.kind == HealthKind::LaunchFailed)
        {
            return DerivedState::LaunchFailed;
        }
        if !self.running {
            return DerivedState::Exited;
        }
        if let Some(h) = &self.health {
            return if h.kind.is_quota() {
                DerivedState::RateLimited
            } else {
                DerivedState::Error
            };
        }
        if self.pending_decision.is_some() {
            return DerivedState::AwaitingDecision;
        }
        if self.working {
            DerivedState::Working
        } else {
            DerivedState::Idle
        }
    }

    pub fn screen_text(&self, max_lines: usize) -> String {
        self.screen.screen_text(max_lines)
    }

    pub fn snapshot(&self) -> Snapshot {
        self.screen.snapshot()
    }

    /// PTY 来了一块输出。
    pub fn on_output(&mut self, bytes: &[u8], now: Instant) {
        if !self.running {
            return;
        }
        self.screen.feed(bytes);
        let replies = self.screen.take_replies();
        if !replies.is_empty() {
            // 终端回复（DA1 / CPR / OSC 11…）先回给子进程：它可能正卡着等。
            self.actions.push(Action::Write(replies));
        }
        self.last_output_at = Some(now);
        if !self.working {
            self.set_working(true, now);
        }
        if let Some(h) = self.quota.feed(bytes) {
            self.set_health(Some(h), now);
        }
        if self.rate_limit.feed(bytes, now) {
            self.answer_rate_limit_menu(now);
        }
        if self.resume_watch.is_some() {
            self.resume_tail.feed(bytes);
        }
        self.check_resume_refusal(now);
        self.step_delivery(now);
    }

    /// 定时拍（0.6s 节拍，或 `next_wakeup` 到点）。
    pub fn on_tick(&mut self, now: Instant) {
        if !self.running {
            return;
        }
        while self.keys.front().is_some_and(|(at, _)| *at <= now) {
            let (_, bytes) = self.keys.pop_front().expect("刚看过非空");
            self.actions.push(Action::Write(bytes));
        }
        let working = self
            .last_output_at
            .is_some_and(|at| now.saturating_duration_since(at) < self.config.working_quiet);
        if working != self.working {
            self.set_working(working, now);
        }
        if quota_recovered(self.health.as_ref(), self.quota_at, self.working_since, now) {
            self.clear_quota_health(now);
        }
        self.check_launch(now);
        self.check_resume_refusal(now);
        let rows = self.screen.screen_rows();
        match self.tracker.poll(now, Some(&rows)) {
            Some(DecisionEvent::Appeared(d)) => {
                self.pending_decision = Some(d.clone());
                self.event(SessionEvent::Decision(DecisionEvent::Appeared(d)));
            }
            Some(DecisionEvent::Cleared) => self.clear_decision(),
            None => {}
        }
        self.step_delivery(now);
    }

    /// 投一段正文（排队，一次只投一笔）。进程已退出则当场报没送到。
    pub fn send(&mut self, id: u64, text: &str, now: Instant) {
        if text.is_empty() {
            return;
        }
        let pending = PendingText {
            id,
            subject: Subject::Steer,
            text: text.to_string(),
        };
        if !self.running {
            let detail = format!(
                "一条指令没送出去：session 已经不在跑了。开头是「{}」。需要重起或改派。",
                preview(text)
            );
            self.finish(pending, DeliveryOutcome::Abandoned(detail.clone()), now);
            self.set_health(
                Some(Health::new(HealthKind::PromptUndelivered, detail)),
                now,
            );
            return;
        }
        self.queue.push_back(pending);
        self.start_next(now);
    }

    /// 原样写字节（nudge / 人在终端里按键）。
    pub fn raw(&mut self, bytes: &[u8]) {
        if self.running && !bytes.is_empty() {
            self.actions.push(Action::Write(bytes.to_vec()));
        }
    }

    pub fn resize(&mut self, cols: u16, lines: u16) {
        if self.screen.resize(cols as usize, lines as usize) {
            self.actions.push(Action::Resize { cols, lines });
        }
    }

    /// 认证探测的结果。
    pub fn on_auth(&mut self, state: AuthState, now: Instant) {
        if !self.running {
            return;
        }
        let next = state.apply(self.health.clone());
        if next != self.health {
            self.set_health(next, now);
        }
    }

    /// 额度重置唤醒到点后由上层调：清掉额度类 health 并重新武装扫描器。
    pub fn clear_quota_health(&mut self, now: Instant) {
        self.quota.rearm();
        if self.health.as_ref().is_some_and(|h| h.kind.is_quota()) {
            self.set_health(None, now);
        }
    }

    /// 我们要停它了（驱动层随后发信号）。没投出去的不报 health——是人叫停的。
    pub fn stop(&mut self, now: Instant) {
        if !self.running {
            return;
        }
        self.stopped_by_user = true;
        self.launch_settled = true;
        self.abandon("session 被停掉", false, now);
    }

    /// 子进程已退出（驱动层收到了退出状态）。
    pub fn on_exit(&mut self, status: ExitStatusKind, now: Instant) {
        if !self.running {
            return;
        }
        if !self.launch_settled && !self.stopped_by_user {
            let verdict = launch_verdict(
                true,
                false,
                true,
                self.last_output_at.is_some(),
                now.saturating_duration_since(self.started_at),
                self.config.launch_deadline,
            );
            let underlying = match &status {
                ExitStatusKind::Code(c) => format!("exit {c}"),
                ExitStatusKind::Signal(s) => format!("signal {s}"),
            };
            if let Some(h) = launch_failure(verdict, self.config.launch_deadline, Some(&underlying))
            {
                self.set_health(Some(h), now);
            }
            self.launch_settled = true;
        }
        let stopped = self.stopped_by_user;
        self.abandon("进程退出", !stopped, now);
        let hit_limit = self
            .quota_at
            .is_some_and(|at| now.saturating_duration_since(at) <= self.config.hit_limit_window);
        self.running = false;
        self.event(SessionEvent::Exited(ExitInfo {
            status,
            stopped_by_user: stopped,
            hit_limit,
        }));
    }

    // ---- 内部 ----

    fn event(&mut self, e: SessionEvent) {
        self.actions.push(Action::Event(e));
    }

    fn set_working(&mut self, working: bool, now: Instant) {
        self.working = working;
        self.working_since = working.then_some(now);
        self.event(SessionEvent::Working(working));
    }

    fn set_health(&mut self, health: Option<Health>, now: Instant) {
        match &health {
            Some(h) if h.kind.is_quota() => {
                self.quota_at = Some(now);
                self.working_since = None;
            }
            None => self.quota_at = None,
            Some(_) => {}
        }
        self.health = health.clone();
        self.event(SessionEvent::Health(health));
    }

    fn clear_decision(&mut self) {
        if self.pending_decision.take().is_some() {
            self.event(SessionEvent::Decision(DecisionEvent::Cleared));
        }
    }

    fn answer_rate_limit_menu(&mut self, now: Instant) {
        self.actions.push(Action::Write(b"1".to_vec()));
        self.schedule(now + self.config.rate_limit_answer_gap, b"\r".to_vec());
        if self.health.as_ref().map(|h| h.kind) != Some(HealthKind::RateLimited) {
            self.set_health(
                Some(Health::new(
                    HealthKind::RateLimited,
                    "Claude Code 撞到额度上限（rate-limit 菜单已自动应答「Stop and wait」）——等额度窗口重置后自动继续。",
                )),
                now,
            );
        }
    }

    fn schedule(&mut self, at: Instant, bytes: Vec<u8>) {
        let pos = self.keys.partition_point(|(t, _)| *t <= at);
        self.keys.insert(pos, (at, bytes));
    }

    fn check_launch(&mut self, now: Instant) {
        if self.launch_settled {
            return;
        }
        let verdict = launch_verdict(
            true,
            true,
            true,
            self.last_output_at.is_some(),
            now.saturating_duration_since(self.started_at),
            self.config.launch_deadline,
        );
        match verdict {
            LaunchVerdict::Alive => self.launch_settled = true,
            v if v.is_terminal() => {
                self.launch_settled = true;
                let h = launch_failure(v, self.config.launch_deadline, None);
                self.set_health(h, now);
            }
            _ => {}
        }
    }

    fn check_resume_refusal(&mut self, now: Instant) {
        let Some((id, deadline)) = &self.resume_watch else {
            return;
        };
        if now > *deadline {
            self.resume_watch = None;
            return;
        }
        let id = id.clone();
        let text = self.screen.screen_text(usize::MAX);
        let said =
            resume_rejection(&text, &id).or_else(|| resume_rejection(self.resume_tail.tail(), &id));
        if let Some(said) = said {
            self.resume_watch = None;
            self.event(SessionEvent::ResumeRefused { id, said });
        }
    }

    fn start_next(&mut self, now: Instant) {
        if self.in_flight.is_some() || !self.running {
            return;
        }
        if let Some(pending) = self.queue.pop_front() {
            let machine = Delivery::new(self.config.delivery, pending.subject, now);
            self.in_flight = Some(InFlight { pending, machine });
            self.step_delivery(now);
        }
    }

    fn step_delivery(&mut self, now: Instant) {
        // 提交三拍（空格 / 退格 / 回车）没发完之前不回读：中间那一帧（多了个空格）不作数。
        if !self.keys.is_empty() {
            return;
        }
        let Some(flight) = self.in_flight.as_mut() else {
            return;
        };
        let rows = self.screen.screen_rows();
        let row = input_row(&rows);
        let step = flight.machine.step(Observation {
            input_row: row.as_deref(),
            body_visible: body_tail_visible(&flight.pending.text, &rows),
            dialog_present: blocking_dialog(&rows).is_some(),
            now,
        });
        match step {
            Step::Idle => {}
            Step::WriteBody { attempt } => {
                let mut bytes = Vec::new();
                if attempt > 1 {
                    // 重投前清掉输入行：上一笔若落了一半，两笔会叠成乱码。
                    bytes.push(0x15);
                }
                bytes.extend_from_slice(sanitize_text(&flight.pending.text).as_bytes());
                self.actions.push(Action::Write(bytes));
            }
            Step::Submit { .. } => {
                // 正文是一次灌进去的，claude 会当成粘贴：单发回车可能只结束粘贴而不提交。
                // 先补一个空格再删掉（输入行判据 trim，不影响判定），隔拍再回车。
                self.actions.push(Action::Write(b" ".to_vec()));
                let gap = self.config.submit_key_gap;
                self.schedule(now + gap, vec![0x7f]);
                self.schedule(now + gap * 2, b"\r".to_vec());
            }
            Step::BlockedByDialog(detail) => self.report(DeliveryOutcome::Blocked(detail), now),
            Step::NotReady(detail) => self.report(DeliveryOutcome::NotReady(detail), now),
            Step::Undelivered(detail) => self.report(DeliveryOutcome::Undelivered(detail), now),
            Step::Unsubmitted(detail) => self.report(DeliveryOutcome::Unsubmitted(detail), now),
            Step::Delivered => self.report(DeliveryOutcome::Delivered, now),
        }
    }

    /// 投递机给了一个报告：非终局只报（翻 health）；终局收掉、清过期 health、取下一笔。
    fn report(&mut self, outcome: DeliveryOutcome, now: Instant) {
        let Some(flight) = self.in_flight.as_ref() else {
            return;
        };
        let kind = match flight.pending.subject {
            Subject::Brief => HealthKind::BriefUndelivered,
            Subject::Steer => HealthKind::PromptUndelivered,
        };
        let pending = flight.pending.clone();
        match &outcome {
            DeliveryOutcome::Delivered => {
                if self.health.as_ref().map(|h| h.kind) == Some(kind) {
                    self.set_health(None, now);
                }
            }
            DeliveryOutcome::Blocked(d)
            | DeliveryOutcome::NotReady(d)
            | DeliveryOutcome::Undelivered(d)
            | DeliveryOutcome::Unsubmitted(d)
            | DeliveryOutcome::Abandoned(d) => {
                self.set_health(Some(Health::new(kind, d.clone())), now);
            }
        }
        if outcome.is_final() {
            self.in_flight = None;
            self.finish(pending, outcome, now);
            self.start_next(now);
        } else {
            self.event(SessionEvent::Delivery {
                id: pending.id,
                subject: pending.subject,
                outcome,
            });
        }
    }

    fn finish(&mut self, pending: PendingText, outcome: DeliveryOutcome, _now: Instant) {
        self.event(SessionEvent::Delivery {
            id: pending.id,
            subject: pending.subject,
            outcome,
        });
    }

    /// 手上这笔和排队的都投不出去了。`report`：给运行中的指令留一条 health。
    fn abandon(&mut self, reason: &str, report: bool, now: Instant) {
        let lost = self.take_pending();
        self.keys.clear();
        for p in &lost {
            self.finish(
                p.clone(),
                DeliveryOutcome::Abandoned(reason.to_string()),
                now,
            );
        }
        let steers: Vec<&PendingText> = lost
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
        self.clear_decision();
        if self.working {
            self.set_working(false, now);
        }
    }
}

/// 正文写进 PTY 之前去掉「会被当成按键」的字节：CR（单独或 CRLF）当换行——单独一个 CR
/// 就是「提交」；其余控制字符（ESC 会打断回合、Ctrl-C 会中止、NUL…）换成看得见的
/// Unicode 控制图形（U+2400 起），换行与制表符原样保留。白板里贴进来的终端输出常带这些。
pub fn sanitize_text(text: &str) -> String {
    text.replace("\r\n", "\n")
        .chars()
        .map(|c| match c {
            '\r' => '\n',
            '\n' | '\t' => c,
            '\u{7f}' => '\u{2421}',
            c if (c as u32) < 0x20 => char::from_u32(0x2400 + c as u32).unwrap_or('\u{fffd}'),
            c if ('\u{80}'..='\u{9f}').contains(&c) => '\u{fffd}',
            c => c,
        })
        .collect()
}

/// 一眼能认出是哪条的前 40 个字符（换行压成空格）。
pub fn preview(text: &str) -> String {
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= 40 {
        one_line
    } else {
        one_line.chars().take(40).collect::<String>() + "…"
    }
}

/// 屏上有没有 claude 拒绝续跑**这个** id 的原话；有则返回那一行。
pub fn resume_rejection(screen_text: &str, resumed_id: &str) -> Option<String> {
    let id = resumed_id.trim();
    if id.is_empty() {
        return None;
    }
    screen_text
        .lines()
        .map(str::trim)
        .find(|l| l.contains(NO_CONVERSATION_MARKER) && l.contains(id))
        .map(str::to_string)
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p crewd-exec`
Expected: `72 passed`。

- [ ] **Step 5: 验这几把尺子会红**

每刀单独做、做完改回（编辑器撤销），最后再跑一次全绿：
1. `delivery.rs` 里 `Timing::default()` 的 `ready_settle: Duration::from_millis(300)` 临时改成 `Duration::ZERO` → `cargo test -p crewd-exec --lib` → 红 3 条：`recorded_trust_screen_blocks_the_brief`（裁定 4 的真字节证据）、`retry_clears_the_input_row_with_ctrl_u`、`steers_queue_behind_the_one_in_flight`（后两条按 300ms 节奏写的）。
2. `step_delivery` 开头 `if !self.keys.is_empty() { return; }` 临时删掉 → 红 1 条：`row_change_mid_submit_sequence_is_not_success`。
3. `check_resume_refusal` 里把 `.or_else(|| resume_rejection(self.resume_tail.tail(), &id))` 临时删掉 → 红 1 条：`refusal_is_recognised_even_when_wrapped_on_a_narrow_screen`。
4. `Step::WriteBody` 分支里 `sanitize_text(&flight.pending.text)` 临时换成 `flight.pending.text.clone()` → 红 2 条：`carriage_returns_inside_text_never_submit_early`、`control_characters_in_text_are_made_inert`。

- [ ] **Step 6: 全量检查并提交**

Run: `./scripts/check.sh` → 0。

```bash
git add crates/exec/src/lib.rs crates/exec/src/session.rs
git commit -m "crewd-exec: sans-IO session core (delivery, health, decisions, exit) driven by screen and injected time

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 7: 孤儿登记与回收（`registry`）

Swift `SessionOrphanReaper` 的移植 + Linux 那一半（裁定 10）。判定是纯函数（**只有启动时刻完全一致才 `Reap`**，pid 复用绝不动手，读不到身份也不动手）；系统调用那一半 `probe` 按 OS 分两份；登记表是 JSON 文件（0600、写临时文件再 rename），坏了报错不当空表；`reap_orphans` 逐条判定、执行、清账（读不到身份的那条留着下次再看）。**调用方必须先拿到数据目录锁**——确认没有别的 crewd 活着，否则会杀掉别人的活会话。

**Files:**
- Create: `crewd/crates/exec/src/registry.rs`
- Modify: `crewd/crates/exec/src/lib.rs`、`crewd/crates/exec/Cargo.toml`、`crewd/Cargo.lock`

**Interfaces:**
- Consumes: 无。
- Produces（`crewd_exec::registry`）:
  - `#[derive(Serialize, Deserialize)] #[serde(rename_all = "camelCase")] pub struct ProcessIdentity { pub pid: i32, pub pgid: i32, pub start_time: u64, pub command: String }`
  - `pub enum ProbeResult { Found(ProcessIdentity), Missing, Unreadable(i32) }`、`fn probe(pid: i32) -> ProbeResult`、`fn parse_proc_stat(&str) -> Option<(i32, u64, String)>`（所有平台都编译）
  - `pub enum OrphanDecision { AlreadyGone, Reap, PidReused(ProcessIdentity), Unreadable(i32) }`、`fn decide(&ProcessIdentity, &ProbeResult) -> OrphanDecision`、`fn describe(session_id: &str, &ProcessIdentity, &OrphanDecision) -> String`、`fn apply(&OrphanDecision, &ProcessIdentity) -> bool`
  - `pub struct ProcessRegistry`：`fn load(&Path) -> io::Result<Self>`、`fn record(&self, session_id: &str, ProcessIdentity) -> io::Result<()>`、`fn forget(&self, session_id: &str) -> io::Result<()>`、`fn entries(&self) -> Vec<(String, ProcessIdentity)>`（内部 `Mutex`，可放进 `Arc` 共享）
  - `pub struct OrphanReport { pub session_id: String, pub decision: OrphanDecision, pub message: String }`、`fn reap_orphans(&ProcessRegistry) -> io::Result<Vec<OrphanReport>>`

- [ ] **Step 1: 加依赖**

```bash
cargo add --offline -p crewd-exec serde --features derive
```

- [ ] **Step 2: 写失败的测试**

`crates/exec/src/lib.rs` 整个换成：

```rust
//! crewd 的执行器：起 agent 进程（PTY）、读它的屏幕、投递正文、认健康状态、收尸与恢复。
//! 其余部分只经 `driver::SessionHandle` 这一个窄接口与它交互（spec §13 Q12）。
pub mod delivery;
pub mod env;
pub mod health;
pub mod launch;
pub mod pty;
pub mod registry;
pub mod session;
```

`crates/exec/src/registry.rs`：先只放测试模块。`reaper_kills_a_live_orphan_group_but_not_a_reused_pid` 里两个受害者进程**都自成进程组**（Global Constraints；写计划时有一版把「无辜进程」留在测试自己的进程组里，变异后 killpg 把 cargo test 连同终端一起杀了）：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    fn identity(pid: i32, start: u64) -> ProcessIdentity {
        ProcessIdentity {
            pid,
            pgid: pid,
            start_time: start,
            command: "claude".to_string(),
        }
    }

    #[test]
    fn decision_requires_exact_start_time() {
        let rec = identity(4242, 1_700_000_000_123_456);
        assert_eq!(
            decide(&rec, &ProbeResult::Missing),
            OrphanDecision::AlreadyGone
        );
        assert_eq!(
            decide(&rec, &ProbeResult::Found(rec.clone())),
            OrphanDecision::Reap
        );
        let moved = identity(4242, 1_700_000_000_123_457);
        assert_eq!(
            decide(&rec, &ProbeResult::Found(moved.clone())),
            OrphanDecision::PidReused(moved),
            "只差一微秒也判复用"
        );
        assert_eq!(
            decide(&rec, &ProbeResult::Unreadable(1)),
            OrphanDecision::Unreadable(1)
        );
    }

    #[test]
    fn only_reap_sends_a_signal_and_reuse_says_hands_off() {
        let rec = identity(i32::MAX, 1);
        assert!(!apply(&OrphanDecision::AlreadyGone, &rec));
        assert!(!apply(
            &OrphanDecision::PidReused(identity(i32::MAX, 2)),
            &rec
        ));
        assert!(!apply(&OrphanDecision::Unreadable(1), &rec));
        let msg = describe(
            "w-1",
            &rec,
            &OrphanDecision::PidReused(identity(i32::MAX, 2)),
        );
        assert!(msg.contains("没有动它"), "{msg}");
    }

    #[test]
    fn proc_stat_parsing_survives_odd_command_names() {
        let mut fields = vec!["S".to_string(), "1".to_string(), "777".to_string()];
        fields.extend((6..=21).map(|n| n.to_string()));
        fields.push("987654".to_string());
        fields.extend((23..=52).map(|n| n.to_string()));
        let line = format!("4242 (we ird) (name) {}", fields.join(" "));
        assert_eq!(
            parse_proc_stat(&line),
            Some((777, 987654, "we ird) (name".to_string()))
        );
        assert_eq!(parse_proc_stat("garbage"), None);
        assert_eq!(parse_proc_stat("1 (x) S 1"), None);
    }

    #[test]
    fn probe_sees_this_process_and_misses_a_reaped_child() {
        let me = std::process::id() as i32;
        let ProbeResult::Found(id) = probe(me) else {
            panic!("读不到自己");
        };
        assert_eq!(id.pid, me);
        let pgid = rustix::process::getpgid(None)
            .unwrap()
            .as_raw_nonzero()
            .get();
        assert_eq!(id.pgid, pgid);
        assert!(id.start_time > 0);
        assert_eq!(probe(me), ProbeResult::Found(id), "同一进程读两次完全一致");

        let mut child = Command::new("/bin/sh")
            .arg("-c")
            .arg("exit 0")
            .spawn()
            .unwrap();
        let pid = child.id() as i32;
        child.wait().unwrap();
        assert_eq!(probe(pid), ProbeResult::Missing);
        assert_eq!(probe(0), ProbeResult::Missing);
    }

    #[test]
    fn reaper_kills_a_live_orphan_group_but_not_a_reused_pid() {
        let dir = tempfile::tempdir().unwrap();
        let reg = ProcessRegistry::load(&dir.path().join("registry.json")).unwrap();
        // 一个自成进程组的「孤儿」。
        let mut orphan = Command::new("/bin/sh");
        orphan.arg("-c").arg("sleep 30");
        std::os::unix::process::CommandExt::process_group(&mut orphan, 0);
        let mut orphan = orphan.spawn().unwrap();
        let pid = orphan.id() as i32;
        let ProbeResult::Found(real) = probe(pid) else {
            panic!("刚起的进程读不到");
        };
        reg.record("live", real.clone()).unwrap();
        // 同一个 pid、但登记的启动时刻差一点：必须不动。它也自成进程组——万一判定写错了，
        // killpg 杀的是它自己那一组，而不是跑测试的这一组（会把 cargo test 连同终端一起带走）。
        let mut innocent = Command::new("/bin/sh");
        innocent.arg("-c").arg("sleep 30");
        std::os::unix::process::CommandExt::process_group(&mut innocent, 0);
        let mut innocent = innocent.spawn().unwrap();
        let ipid = innocent.id() as i32;
        let ProbeResult::Found(mut fake) = probe(ipid) else {
            panic!();
        };
        fake.start_time += 1;
        reg.record("reused", fake).unwrap();

        let reports = reap_orphans(&reg).unwrap();
        assert_eq!(reports.len(), 2);
        let status = orphan.wait().unwrap();
        assert_eq!(
            std::os::unix::process::ExitStatusExt::signal(&status),
            Some(libc::SIGKILL)
        );
        assert!(
            innocent.try_wait().unwrap().is_none(),
            "复用的 pid 绝不能被杀"
        );
        assert!(reg.entries().is_empty(), "处理过的都清账");
        innocent.kill().unwrap();
        innocent.wait().unwrap();
    }

    #[test]
    fn registry_persists_privately_and_refuses_corruption() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("registry.json");
        let reg = ProcessRegistry::load(&path).unwrap();
        reg.record("a", identity(10, 1)).unwrap();
        reg.record("b", identity(11, 2)).unwrap();
        reg.forget("a").unwrap();
        reg.forget("never").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let again = ProcessRegistry::load(&path).unwrap();
        assert_eq!(again.entries(), vec![("b".to_string(), identity(11, 2))]);
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("startTime"), "camelCase：{raw}");
        std::fs::write(&path, b"{not json").unwrap();
        assert!(
            ProcessRegistry::load(&path).is_err(),
            "坏掉的登记表不能当空表"
        );
    }
}
```

- [ ] **Step 3: 跑测试确认失败**

Run: `cargo test -p crewd-exec registry::`
Expected: 编译失败，``cannot find type `ProcessIdentity` in this scope``。

- [ ] **Step 4: 实现**

放在 `crates/exec/src/registry.rs` 的测试模块之前：

```rust
//! 孤儿进程登记与回收（Swift `SessionOrphanReaper` 的移植）。
//!
//! crewd 崩了 / 被杀之后，它起的 agent 进程可能还活着。重启时按登记表回收——
//! 但 **pid 会复用**：只有「登记的启动时刻」与「这个 pid 现在的启动时刻」完全一致才动手，
//! 宁可留一个孤儿，也不误杀一个无辜进程。
//!
//! 启动时刻：macOS 取 `proc_pidinfo(PROC_PIDTBSDINFO)` 的秒 + 微秒（与 sysctl
//! `KERN_PROC` 的 `p_starttime` 是同一个内核字段；libc 没给 macOS 导出 `kinfo_proc`），
//! Linux 取 `/proc/<pid>/stat` 第 22 字段（开机以来的时钟滴答）。两者都折成一个 `u64`，
//! 只在同一台机器上比较。
use std::collections::BTreeMap;
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessIdentity {
    pub pid: i32,
    pub pgid: i32,
    /// 这把锁：macOS = 微秒时间戳，Linux = 开机以来的时钟滴答。
    pub start_time: u64,
    /// 进程短名。只给人看，不参与判定（会截断、会重名）。
    pub command: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProbeResult {
    Found(ProcessIdentity),
    Missing,
    /// 系统调用答不上来（如权限）——不是「不存在」。
    Unreadable(i32),
}

/// 读 `pid` 当前的身份。
pub fn probe(pid: i32) -> ProbeResult {
    if pid <= 0 {
        return ProbeResult::Missing;
    }
    os_probe(pid)
}

#[cfg(target_os = "macos")]
fn os_probe(pid: i32) -> ProbeResult {
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: 缓冲区大小与 flavor 对应的结构体一致；内核只写满 `size` 字节。
    let n = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    if n != size {
        let errno = io::Error::last_os_error().raw_os_error().unwrap_or(0);
        return if errno == libc::ESRCH || n == 0 && errno == 0 {
            ProbeResult::Missing
        } else {
            ProbeResult::Unreadable(errno)
        };
    }
    // SAFETY: 上面确认内核写满了整个结构体。
    let info = unsafe { info.assume_init() };
    let comm: Vec<u8> = info
        .pbi_comm
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u8)
        .collect();
    ProbeResult::Found(ProcessIdentity {
        pid,
        pgid: info.pbi_pgid as i32,
        start_time: info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec,
        command: String::from_utf8_lossy(&comm).into_owned(),
    })
}

#[cfg(not(target_os = "macos"))]
fn os_probe(pid: i32) -> ProbeResult {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(content) => match parse_proc_stat(&content) {
            Some((pgid, start_time, command)) => ProbeResult::Found(ProcessIdentity {
                pid,
                pgid,
                start_time,
                command,
            }),
            None => ProbeResult::Unreadable(0),
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => ProbeResult::Missing,
        Err(e) => ProbeResult::Unreadable(e.raw_os_error().unwrap_or(0)),
    }
}

/// 解析 Linux `/proc/<pid>/stat`：返回（第 5 字段 pgrp，第 22 字段 starttime，comm）。
/// comm 里可能有空格和括号，所以按**最后一个** `)` 切。所有平台都编译，便于单测。
pub fn parse_proc_stat(content: &str) -> Option<(i32, u64, String)> {
    let open = content.find('(')?;
    let close = content.rfind(')')?;
    let command = content.get(open + 1..close)?.to_string();
    let rest: Vec<&str> = content.get(close + 1..)?.split_whitespace().collect();
    // rest[0] 是第 3 字段（state），所以第 N 字段在 rest[N - 3]。
    let pgid = rest.get(5 - 3)?.parse().ok()?;
    let start = rest.get(22 - 3)?.parse().ok()?;
    Some((pgid, start, command))
}

/// 一条登记的处置。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OrphanDecision {
    AlreadyGone,
    /// 启动时刻对得上：确认是我们的孤儿，杀进程组。
    Reap,
    /// pid 被复用了：绝不动手。
    PidReused(ProcessIdentity),
    /// 读不到：不动手，留给人。
    Unreadable(i32),
}

pub fn decide(recorded: &ProcessIdentity, current: &ProbeResult) -> OrphanDecision {
    match current {
        ProbeResult::Missing => OrphanDecision::AlreadyGone,
        ProbeResult::Unreadable(errno) => OrphanDecision::Unreadable(*errno),
        ProbeResult::Found(now) if now.start_time == recorded.start_time => OrphanDecision::Reap,
        ProbeResult::Found(now) => OrphanDecision::PidReused(now.clone()),
    }
}

/// 给人看的一句话。`PidReused` / `Unreadable` 必须说清「我没动它」。
pub fn describe(session_id: &str, recorded: &ProcessIdentity, decision: &OrphanDecision) -> String {
    let pid = recorded.pid;
    match decision {
        OrphanDecision::AlreadyGone => {
            format!("「{session_id}」的进程（pid {pid}）已经不在了，无需处理。")
        }
        OrphanDecision::Reap => format!(
            "「{session_id}」的进程（pid {pid}）启动时刻与登记一致，已回收其进程组 {}。",
            recorded.pgid
        ),
        OrphanDecision::PidReused(now) => format!(
            "「{session_id}」登记的 pid {pid} 现在属于另一个进程（{}，启动时刻 {} ≠ 登记的 {}）——pid 被复用了，我没有动它。",
            now.command, now.start_time, recorded.start_time
        ),
        OrphanDecision::Unreadable(errno) => format!(
            "「{session_id}」登记的 pid {pid} 读不到身份（errno {errno}），我没有动它；要不要处理请人来定。"
        ),
    }
}

/// 执行处置：只有 `Reap` 发信号（先 killpg，失败再单杀）。返回是否发了信号。
pub fn apply(decision: &OrphanDecision, recorded: &ProcessIdentity) -> bool {
    if *decision != OrphanDecision::Reap {
        return false;
    }
    let kill = rustix::process::Signal::KILL;
    let group = rustix::process::Pid::from_raw(if recorded.pgid > 0 {
        recorded.pgid
    } else {
        recorded.pid
    });
    let sent = group.is_some_and(|g| rustix::process::kill_process_group(g, kill).is_ok());
    if !sent && let Some(p) = rustix::process::Pid::from_raw(recorded.pid) {
        let _ = rustix::process::kill_process(p, kill);
    }
    true
}

/// 「如果我下一秒崩了，回来之后该去看哪些 pid」。JSON 文件，0600，整份原子替换。
#[derive(Debug)]
pub struct ProcessRegistry {
    path: PathBuf,
    entries: Mutex<BTreeMap<String, ProcessIdentity>>,
}

impl ProcessRegistry {
    /// 读登记表。文件不存在 = 空表；存在但解析不了 = 错误（不能当空表，否则孤儿就漏了）。
    pub fn load(path: &Path) -> io::Result<Self> {
        let entries = match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{}: {e}", path.display()),
                )
            })?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => return Err(e),
        };
        Ok(Self {
            path: path.to_path_buf(),
            entries: Mutex::new(entries),
        })
    }

    pub fn record(&self, session_id: &str, identity: ProcessIdentity) -> io::Result<()> {
        let mut entries = self.entries.lock().expect("登记表锁中毒");
        entries.insert(session_id.to_string(), identity);
        self.persist(&entries)
    }

    pub fn forget(&self, session_id: &str) -> io::Result<()> {
        let mut entries = self.entries.lock().expect("登记表锁中毒");
        if entries.remove(session_id).is_none() {
            return Ok(());
        }
        self.persist(&entries)
    }

    pub fn entries(&self) -> Vec<(String, ProcessIdentity)> {
        let entries = self.entries.lock().expect("登记表锁中毒");
        entries
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    fn persist(&self, entries: &BTreeMap<String, ProcessIdentity>) -> io::Result<()> {
        let bytes = serde_json::to_vec_pretty(entries).map_err(io::Error::other)?;
        let tmp = self.path.with_extension("json.tmp");
        let _ = std::fs::remove_file(&tmp);
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        io::Write::write_all(&mut f, &bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, &self.path)
    }
}

/// 一条回收结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrphanReport {
    pub session_id: String,
    pub decision: OrphanDecision,
    pub message: String,
}

/// 启动时调用（必须持有数据目录锁，确认没有别的 crewd 活着）：逐条判定、执行、清账。
/// 读不到身份的那条留在表里，下次再看。
pub fn reap_orphans(registry: &ProcessRegistry) -> io::Result<Vec<OrphanReport>> {
    let mut reports = Vec::new();
    for (session_id, recorded) in registry.entries() {
        let decision = decide(&recorded, &probe(recorded.pid));
        apply(&decision, &recorded);
        if !matches!(decision, OrphanDecision::Unreadable(_)) {
            registry.forget(&session_id)?;
        }
        reports.push(OrphanReport {
            message: describe(&session_id, &recorded, &decision),
            session_id,
            decision,
        });
    }
    Ok(reports)
}
```

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -p crewd-exec`
Expected: `78 passed`。

- [ ] **Step 6: 验这把尺子会红**

`decide` 里 `ProbeResult::Found(now) if now.start_time == recorded.start_time => OrphanDecision::Reap,` 临时改成 `ProbeResult::Found(now) if now.start_time == recorded.start_time || true => OrphanDecision::Reap,` → `cargo test -p crewd-exec --lib registry` → 红 2 条：`decision_requires_exact_start_time`、`reaper_kills_a_live_orphan_group_but_not_a_reused_pid`（无辜进程被杀）。改回，再跑一次全绿。**先确认测试里两个子进程都调了 `process_group(…, 0)` 再做这一刀。**

- [ ] **Step 7: 全量检查并提交**

Run: `./scripts/check.sh` → 0。

```bash
git add Cargo.lock crates/exec/Cargo.toml crates/exec/src/lib.rs crates/exec/src/registry.rs
git commit -m "crewd-exec: orphan registry and start-time verified reaper (macOS proc_pidinfo, Linux /proc)

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 8: 执行器类型字段（spec §16）

spec §16：「session 的执行器类型字段设计为 `claude` / `codex` / `acp:<registry-id>`，M2b 起就按此建，避免日后迁移」。现在的列叫 `agent_kind`、值是旧 Swift 的 `claude_code` / `codex` / `unknown`。schema v2 改名 + 改值（裁定 12），proto、旧数据导入、契约文件一起改——只有一个事实源，不留旧名字。这一步不碰 `crewd-exec`。

已有的库（M1 导入出来的）下次打开时自动迁到 v2；v2 的库旧二进制拒绝打开（`数据库版本 2 比本程序支持的 1 新`），这是 M1 就有的规矩。

**Files:**
- Modify: `crewd/crates/store/src/schema.rs`、`crewd/crates/store/src/sessions.rs`、`crewd/crates/store/src/lib.rs`（测试）、`crewd/crates/proto/src/model.rs`、`crewd/crates/legacy/src/sessions.rs`、`crewd/crates/legacy/src/lib.rs`（测试）、`crewd/schema/crewd-api.schema.json`

**Interfaces:**
- Consumes: 无。
- Produces:
  - `crewd_store::schema::SCHEMA_VERSION == 2`；表 `sessions` 的列 `executor TEXT NOT NULL`（原 `agent_kind`）
  - `crewd_proto::Session { …, pub executor: String, … }`（原 `agent_kind`；线上 JSON 字段 `executor`）
  - `crewd_legacy::sessions::executor_of(kind: &str) -> String`（`claude_code` → `claude`，其余原样）

- [ ] **Step 1: 写失败的测试**

在 `crates/store/src/schema.rs` 文件末尾追加（用 v1 的建表语句造一个旧库，迁移后看列名和值）：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v1_database_is_migrated_to_executor_vocabulary() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(V1).unwrap();
        conn.execute_batch(
            "INSERT INTO crews(id,title,created_at,updated_at) VALUES('c','t','x','x');
             INSERT INTO sessions(id,crew_id,role,agent_kind,updated_at) VALUES
               ('a','c','worker','claude_code','x'),
               ('b','c','worker','codex','x'),
               ('d','c','worker','unknown','x');",
        )
        .unwrap();
        migrate(&conn).unwrap();
        let v: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, 2);
        let mut stmt = conn
            .prepare("SELECT id, executor FROM sessions ORDER BY id")
            .unwrap();
        let rows: Vec<(String, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            rows,
            [
                ("a".to_string(), "claude".to_string()),
                ("b".to_string(), "codex".to_string()),
                ("d".to_string(), "unknown".to_string()),
            ]
        );
        migrate(&conn).unwrap();
    }
}
```

在 `crates/legacy/src/lib.rs` 的导入测试里，把

```rust
        assert_eq!(old.agent_kind, "unknown");
```

换成

```rust
        assert_eq!(old.executor, "unknown");
        assert_eq!(
            cap.executor, "claude",
            "claude_code 改名为 claude（spec §16）"
        );
        assert_eq!(w.executor, "codex");
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p crewd-store -p crewd-legacy`
Expected: crewd-legacy 编译失败（``no field `executor` on type `Session` ``）；单跑 `cargo test -p crewd-store schema` 则是 `v1_database_is_migrated_to_executor_vocabulary` 运行时失败（``assertion `left == right` failed``，`left: 1  right: 2`——还没有 v2 迁移）。

- [ ] **Step 3: 实现**

`crates/store/src/schema.rs`：版本号改成 2，在 `V1` 之后、`migrate` 之前加 `V2`，`migrate` 里补一段：

```rust
pub const SCHEMA_VERSION: i64 = 2;
```

```rust
/// v2（M2b，spec §16）：session 的执行器类型字段。`agent_kind` 改名 `executor`，
/// 词表改成 `claude` / `codex` / `acp:<registry-id>`；旧数据里的 `terminal` / `unknown` 原样保留。
const V2: &str = r#"
BEGIN;
ALTER TABLE sessions RENAME COLUMN agent_kind TO executor;
UPDATE sessions SET executor='claude' WHERE executor='claude_code';
PRAGMA user_version = 2;
COMMIT;
"#;
```

```rust
    if v < 1 {
        conn.execute_batch(V1)?;
    }
    if v < 2 {
        conn.execute_batch(V2)?;
    }
    Ok(())
```

（`V1` 的建表语句**不改**——新库先建 v1 再迁 v2，与老库走同一条路。）

`crates/store/src/sessions.rs`：两条 SQL 里的列名和两处字段跟着改——

```rust
        "INSERT INTO sessions(id,crew_id,role,executor,agent_session_id,display_name,working_directory,model,effort,updated_at,extra)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)
         ON CONFLICT(id) DO UPDATE SET crew_id=excluded.crew_id, role=excluded.role, executor=excluded.executor,
```

```rust
            s.executor,
```

```rust
        "SELECT id,crew_id,role,executor,agent_session_id,display_name,working_directory,model,effort,updated_at
```

```rust
                executor: r.get(3)?,
```

`crates/proto/src/model.rs` 的 `Session`：

```rust
    /// 执行器类型："claude" | "codex" | "acp:<registry-id>"；旧数据导入的还可能是 "terminal" | "unknown"。
    pub executor: String,
```

（替换原来的 `/// "claude_code" | "codex" | "unknown"` 与 `pub agent_kind: String,` 两行。）

`crates/store/src/lib.rs` 测试 `sessions_upsert_and_list` 里 `agent_kind: "claude_code".into(),` 换成 `executor: "claude".into(),`。

`crates/legacy/src/sessions.rs`：在 `pub fn import` 的文档注释之前加

```rust
/// 旧版 runner 种类 → 执行器类型（spec §16）。只有 `claude_code` 改名；`codex`、`terminal`
/// 以及认不出的原样保留。
pub fn executor_of(kind: &str) -> String {
    match kind {
        "claude_code" => "claude".to_string(),
        other => other.to_string(),
    }
}
```

同一文件里：文档注释 `以 agent_kind="unknown" 入库` 改成 `以 executor="unknown" 入库`；`agent_kind: r.kind.clone(),` 换成 `executor: executor_of(&r.kind),`；`agent_kind: "unknown".into(),` 换成 `executor: "unknown".into(),`。

改完 `grep -rn "agent_kind" crates | grep -v captain_agent_kind | grep -v target` 应该只剩 `schema.rs` 里 V1 建表那一行和 V2 的注释 / `RENAME` 语句。

- [ ] **Step 4: 重新生成契约文件**

```bash
cargo run -p crewd -- schema > schema/crewd-api.schema.json
git diff --stat schema/
```

Expected: `1 file changed, 5 insertions(+), 5 deletions(-)`——`Session` 的 `agentKind` 属性换成 `executor`（带新描述），`required` 里同样替换。

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -p crewd-store -p crewd-legacy -p crewd`
Expected: crewd-store 单测 `10 passed`、crewd-legacy 单测 `16 passed`、`import_basic` 与 `e2e`（含契约漂移检查）全绿。

- [ ] **Step 6: 全量检查并提交**

Run: `./scripts/check.sh` → 0。

```bash
git add crates/store/src/schema.rs crates/store/src/sessions.rs crates/store/src/lib.rs crates/proto/src/model.rs crates/legacy/src/sessions.rs crates/legacy/src/lib.rs schema/crewd-api.schema.json
git commit -m "store: schema v2 renames sessions.agent_kind to executor (claude/codex/acp:<id>)

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 9: 会话号账本（`resume` + store + `StoreLedger`）

重启后接回原对话只要一件事：这个 crewd session 上次用的 claude 会话号（spec §13 Q12：重启 / 升级只要求能恢复）。执行器侧是 trait（裁定 11）+ 决策函数；store 侧两个函数读写 `sessions.agent_session_id`（session 行不存在是错误，不当成「没记过」）；server 侧 `StoreLedger` 把两边接起来。

**Files:**
- Create: `crewd/crates/exec/src/resume.rs`、`crewd/crates/server/src/ledger.rs`
- Modify: `crewd/crates/exec/src/lib.rs`、`crewd/crates/store/src/sessions.rs`、`crewd/crates/store/src/lib.rs`（测试）、`crewd/crates/server/src/lib.rs`、`crewd/crates/server/Cargo.toml`、`crewd/Cargo.lock`

**Interfaces:**
- Consumes: Task 2 `launch::{AgentSessionArg, new_agent_session_id}`；Task 8 `Session.executor`。
- Produces:
  - `crewd_exec::resume`：`pub trait AgentIdLedger: Send + Sync { fn recorded(&self) -> io::Result<Option<String>>; fn record(&self, agent_session_id: &str) -> io::Result<()>; }`；`#[derive(Default)] pub struct MemoryLedger`（`fn with(Option<&str>) -> Self`、`fn current(&self) -> Option<String>`）；`fn decide_resume(recorded: Option<&str>) -> AgentSessionArg`
  - `crewd_store::sessions::agent_session_id(conn: &Connection, session_id: &str) -> Result<Option<String>>`、`set_agent_session_id(conn: &Connection, session_id: &str, agent_session_id: &str) -> Result<()>`（顺带更新 `updated_at`）
  - `crewd_server::ledger::StoreLedger::new(db: PathBuf, session_id: impl Into<String>) -> StoreLedger`，`impl AgentIdLedger for StoreLedger`

- [ ] **Step 1: 写失败的测试**

`crates/exec/src/lib.rs` 整个换成：

```rust
//! crewd 的执行器：起 agent 进程（PTY）、读它的屏幕、投递正文、认健康状态、收尸与恢复。
//! 其余部分只经 `driver::SessionHandle` 这一个窄接口与它交互（spec §13 Q12）。
pub mod delivery;
pub mod env;
pub mod health;
pub mod launch;
pub mod pty;
pub mod registry;
pub mod resume;
pub mod session;
```

`crates/exec/src/resume.rs`：先只放测试模块：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recorded_id_always_resumes_without_predicting() {
        assert_eq!(
            decide_resume(Some(" abc ")),
            AgentSessionArg::Resume("abc".into())
        );
    }

    #[test]
    fn no_or_blank_record_starts_fresh_with_a_new_id() {
        for recorded in [None, Some(""), Some("  ")] {
            match decide_resume(recorded) {
                AgentSessionArg::New(id) => assert_eq!(id.len(), 36),
                other => panic!("{other:?}"),
            }
        }
    }

    #[test]
    fn memory_ledger_round_trips() {
        let l = MemoryLedger::default();
        assert_eq!(l.recorded().unwrap(), None);
        l.record("x").unwrap();
        assert_eq!(l.recorded().unwrap().as_deref(), Some("x"));
        assert_eq!(
            MemoryLedger::with(Some("y")).current().as_deref(),
            Some("y")
        );
    }
}
```

`crates/store/src/lib.rs` 测试模块里、`is_unique_violation_only_matches_unique_and_primary_key` 之前加：

```rust
    #[test]
    fn agent_session_id_ledger() {
        let s = Store::open_in_memory().unwrap();
        crews::upsert(s.conn(), &crew("a", None), None).unwrap();
        let ses = Session {
            id: "w-1".into(),
            crew_id: "a".into(),
            role: "worker".into(),
            executor: "claude".into(),
            agent_session_id: None,
            display_name: None,
            working_directory: None,
            model: None,
            effort: None,
            updated_at: "2026-10-01T00:00:00.000Z".into(),
        };
        sessions::upsert(s.conn(), &ses, None).unwrap();
        assert_eq!(sessions::agent_session_id(s.conn(), "w-1").unwrap(), None);
        sessions::set_agent_session_id(s.conn(), "w-1", "uuid-9").unwrap();
        assert_eq!(
            sessions::agent_session_id(s.conn(), "w-1")
                .unwrap()
                .as_deref(),
            Some("uuid-9")
        );
        let list = sessions::list_for_crew(s.conn(), "a").unwrap();
        assert_ne!(
            list[0].updated_at, "2026-10-01T00:00:00.000Z",
            "落账顺带更新时间"
        );
        assert!(sessions::agent_session_id(s.conn(), "nope").is_err());
        assert!(sessions::set_agent_session_id(s.conn(), "nope", "x").is_err());
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p crewd-exec resume::`
Expected: 编译失败，``cannot find function `decide_resume` in this scope``。

Run: `cargo test -p crewd-store agent_session_id`
Expected: 编译失败，``cannot find function `agent_session_id` in module `sessions` ``。

- [ ] **Step 3: 实现执行器侧与 store 侧**

放在 `crates/exec/src/resume.rs` 的测试模块之前：

```rust
//! 重启后接回原对话（spec §13 Q12：重启/升级只要求能恢复）。
//!
//! 会话号账本只回答一个问题：这个 crewd session 上次用的 claude 会话号是什么。
//! 记过就 `--resume`（不预判它还在不在——claude 按会话号全盘找，预判只会挡掉能接上的）；
//! 没记过就生成新号，**起进程前先落账**。claude 拒绝续跑由驱动层从屏幕认出，换新号重起一次。
//!
//! 执行器不依赖存储：账本是个 trait，crewd 用 SQLite 实现（`crewd_server::ledger`），
//! 测试与不落库的命令行用 [`MemoryLedger`]。以后执行器拆成独立进程，这里换成一次 RPC。
use std::io;
use std::sync::Mutex;

use crate::launch::{AgentSessionArg, new_agent_session_id};

pub trait AgentIdLedger: Send + Sync {
    /// 上次记下的 claude 会话号。
    fn recorded(&self) -> io::Result<Option<String>>;
    /// 落账。失败 = 不起进程（起了也接不回来）。
    fn record(&self, agent_session_id: &str) -> io::Result<()>;
}

/// 只在内存里的账本。
#[derive(Debug, Default)]
pub struct MemoryLedger(Mutex<Option<String>>);

impl MemoryLedger {
    pub fn with(id: Option<&str>) -> Self {
        Self(Mutex::new(id.map(str::to_string)))
    }

    pub fn current(&self) -> Option<String> {
        self.0.lock().expect("账本锁中毒").clone()
    }
}

impl AgentIdLedger for MemoryLedger {
    fn recorded(&self) -> io::Result<Option<String>> {
        Ok(self.current())
    }

    fn record(&self, agent_session_id: &str) -> io::Result<()> {
        *self.0.lock().expect("账本锁中毒") = Some(agent_session_id.to_string());
        Ok(())
    }
}

/// 记过（去空白后非空）就续跑，否则新号。
pub fn decide_resume(recorded: Option<&str>) -> AgentSessionArg {
    match recorded.map(str::trim).filter(|id| !id.is_empty()) {
        Some(id) => AgentSessionArg::Resume(id.to_string()),
        None => AgentSessionArg::New(new_agent_session_id()),
    }
}
```

`crates/store/src/sessions.rs`：头两行 `use` 换成

```rust
use crate::{Result, StoreError};
use crewd_proto::Session;
use rusqlite::{Connection, OptionalExtension, params};
```

文件末尾追加：

```rust
/// 这个 session 记下的 agent 会话号（重启时 `--resume` 用）。session 不存在是错误。
pub fn agent_session_id(conn: &Connection, session_id: &str) -> Result<Option<String>> {
    conn.query_row(
        "SELECT agent_session_id FROM sessions WHERE id=?1",
        [session_id],
        |r| r.get(0),
    )
    .optional()?
    .ok_or_else(|| StoreError::Invalid(format!("没有 session {session_id}")))
}

/// 落账：起 agent 进程之前写下它将用的会话号。session 不存在是错误。
pub fn set_agent_session_id(
    conn: &Connection,
    session_id: &str,
    agent_session_id: &str,
) -> Result<()> {
    let n = conn.execute(
        "UPDATE sessions SET agent_session_id=?2, updated_at=?3 WHERE id=?1",
        params![session_id, agent_session_id, crate::now_ts()],
    )?;
    if n == 0 {
        return Err(StoreError::Invalid(format!("没有 session {session_id}")));
    }
    Ok(())
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p crewd-exec && cargo test -p crewd-store`
Expected: crewd-exec 单测 `81 passed`；crewd-store 单测 `11 passed`。

- [ ] **Step 5: server 侧 `StoreLedger`（先测试）**

```bash
cargo add --offline -p crewd-server crewd-exec --path crates/exec
```

`crates/server/src/lib.rs` 的模块声明里加一行（放在 `pub mod hub;` 之后）：

```rust
pub mod ledger;
```

`crates/server/src/ledger.rs`：先只放测试模块：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crewd_proto::{Crew, Session};
    use crewd_store::crews;

    fn seeded(dir: &std::path::Path) -> PathBuf {
        let db = dir.join("crewd.db");
        let s = Store::open(&db).unwrap();
        let crew = Crew {
            id: "c".into(),
            title: "机组".into(),
            number: None,
            builtin: false,
            parent_ids: vec![],
            working_directory: None,
            captain_agent_kind: None,
            hidden_at: None,
            created_at: "2026-10-01T00:00:00.000Z".into(),
            updated_at: "2026-10-01T00:00:00.000Z".into(),
        };
        crews::upsert(s.conn(), &crew, None).unwrap();
        let ses = Session {
            id: "w-1".into(),
            crew_id: "c".into(),
            role: "worker".into(),
            executor: "claude".into(),
            agent_session_id: None,
            display_name: None,
            working_directory: None,
            model: None,
            effort: None,
            updated_at: "2026-10-01T00:00:00.000Z".into(),
        };
        sessions::upsert(s.conn(), &ses, None).unwrap();
        db
    }

    #[test]
    fn records_and_reads_back_through_sqlite() {
        let dir = tempfile::tempdir().unwrap();
        let db = seeded(dir.path());
        let ledger = StoreLedger::new(db.clone(), "w-1");
        assert_eq!(ledger.recorded().unwrap(), None);
        ledger.record("uuid-1").unwrap();
        assert_eq!(
            StoreLedger::new(db, "w-1").recorded().unwrap().as_deref(),
            Some("uuid-1"),
            "换一个实例（= 重启后）照样读得到"
        );
    }

    #[test]
    fn unknown_session_is_an_error_not_a_fresh_start() {
        let dir = tempfile::tempdir().unwrap();
        let db = seeded(dir.path());
        let ledger = StoreLedger::new(db, "ghost");
        assert!(ledger.recorded().is_err());
        assert!(ledger.record("x").is_err());
    }
}
```

Run: `cargo test -p crewd-server ledger`
Expected: 编译失败，``cannot find struct, variant or union type `StoreLedger` ``。

再把实现放在测试模块之前：

```rust
//! claude 会话号账本的 SQLite 实现（`crewd_exec::resume::AgentIdLedger`）。
//! 每次读写各开一条连接：一次启动只读写一两回，不值得跟 RPC 抢 `AppState` 那把锁。
use std::io;
use std::path::PathBuf;

use crewd_exec::resume::AgentIdLedger;
use crewd_store::{Store, sessions};

pub struct StoreLedger {
    db: PathBuf,
    session_id: String,
}

impl StoreLedger {
    /// `db` = `<数据目录>/crewd.db`；`session_id` = crewd 的 session id（行必须已存在）。
    pub fn new(db: PathBuf, session_id: impl Into<String>) -> Self {
        Self {
            db,
            session_id: session_id.into(),
        }
    }
}

impl AgentIdLedger for StoreLedger {
    fn recorded(&self) -> io::Result<Option<String>> {
        let store = Store::open(&self.db).map_err(io::Error::other)?;
        sessions::agent_session_id(store.conn(), &self.session_id).map_err(io::Error::other)
    }

    fn record(&self, agent_session_id: &str) -> io::Result<()> {
        let store = Store::open(&self.db).map_err(io::Error::other)?;
        sessions::set_agent_session_id(store.conn(), &self.session_id, agent_session_id)
            .map_err(io::Error::other)
    }
}
```

Run: `cargo test -p crewd-server`
Expected: 单测 `4 passed`（原 2 条 + ledger 2 条），`socket` 集成测试照旧全绿。

- [ ] **Step 6: 全量检查并提交**

Run: `./scripts/check.sh` → 0。

```bash
git add Cargo.lock crates/exec/src/lib.rs crates/exec/src/resume.rs crates/store/src/sessions.rs crates/store/src/lib.rs crates/server/Cargo.toml crates/server/src/lib.rs crates/server/src/ledger.rs
git commit -m "exec/store/server: agent session id ledger (record before spawn, resume when recorded)

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 10: 驱动层与窄接口（`driver`）

一个会话 = 一个 tokio 任务。`start_claude`：读账本 → `decide_resume` → 新号先落账（失败即 `StartError::Ledger`，不起进程）→ `prepare_claude` 写临时文件 → `build_env_from_process` → `spawn_pty`（失败即 `StartError::Spawn`，带 `SpawnFailed` 的 health）→ 登记表记一笔（写不进去就杀掉刚起的进程、报错）→ 起写任务、认证探测任务 → 发 `Spawned` → 交出句柄和事件流。

任务主循环 `tokio::select!` 七路：PTY 读、子进程退出、命令、0.6s 节拍、`Core::next_wakeup`、停止宽限到点（`killpg(SIGKILL)`，失败再单杀）、认证结果（按会话号过滤，丢掉被拒那次的旧探测）。每一拍之后执行 `Core` 攒下的动作：写 → 写任务（裁定 13）；`Resize` → `tcsetwinsize`；事件 → 发出去，见到 `ResumeRefused` 且没重试过 → 杀掉（或收下已退出的状态）、换新号重起一次、没投出去的正文搬过去、句柄和事件流不变。子进程退出后先把 PTY 里剩下的输出读干净（最多 500ms）再交给 `Core`——claude 拒绝续跑是「打印一行就退出」。

关于这次「读干净」：**macOS 上它不会发生**——XNU 关闭 tty 从端时会等输出被读走（`ttywait`），没人读主端时子进程根本退不掉，所以总是先读到输出、后看到退出。它守的是 Linux（从端关了数据还在、随后 EIO）。对应的单测因此只在 Linux 编译（`#[cfg(all(test, target_os = "linux"))]`）；写计划时在 macOS 上试过，那条测试会永远挂在 `child.wait()` 上。

`DriverConfig` 的缺省：节拍 0.6s、停止宽限 2s、认证 2s 一次、不探测认证（`claude_projects_dir: None`）、不登记（`registry: None`）——调用方（`crewd claude`、M3 的会话管理）按需打开。

**Files:**
- Create: `crewd/crates/exec/src/driver.rs`、`crewd/crates/exec/tests/driver.rs`
- Modify: `crewd/crates/exec/src/lib.rs`

**Interfaces:**
- Consumes: Task 1 `env::build_env_from_process`；Task 2 `launch::{AgentSessionArg, ClaudeSpec, LaunchFiles, new_agent_session_id, prepare_claude}`；Task 3 `pty::{PtyMaster, SpawnSpec, spawn_pty}`；Task 5 `health::{AUTH_POLL_INTERVAL, AuthProbe, AuthState, Health, HealthKind, LaunchVerdict, launch_failure}`；Task 6 `session::{Action, BRIEF_DELIVERY_ID, Core, CoreConfig, DerivedState, ExitStatusKind, PendingText, SessionEvent}`、`delivery::Subject`；Task 7 `registry::{ProbeResult, ProcessRegistry, probe}`；Task 9 `resume::{AgentIdLedger, decide_resume}`。
- Produces（`crewd_exec::driver`，即窄接口）:
  - `#[derive(Clone)] pub struct DriverConfig { pub core: CoreConfig, pub screen: ScreenConfig, pub tick: Duration, pub stop_grace: Duration, pub runtime_dir: PathBuf, pub claude_projects_dir: Option<PathBuf>, pub auth_poll: Duration, pub registry: Option<Arc<ProcessRegistry>> }`，`fn new(runtime_dir: PathBuf) -> Self`
  - `pub struct ClaudeStart { pub session_id: String, pub program: PathBuf, pub child_path: String, pub spec: ClaudeSpec, pub ledger: Arc<dyn AgentIdLedger> }`
  - `pub enum StartError { Ledger(io::Error), Files(io::Error), Spawn { health: Health, source: io::Error }, Registry(io::Error) }`（`Display + Error`）
  - `pub struct Closed;`、`pub struct SessionStatus { pub state: DerivedState, pub working: bool, pub health: Option<Health>, pub pending_decision: Option<PendingDecision>, pub pid: u32, pub agent_session_id: String }`
  - `#[derive(Clone)] pub struct SessionHandle`：`send(&self, impl Into<String>) -> Result<u64, Closed>`、`raw(&self, impl Into<Vec<u8>>) -> Result<(), Closed>`、`resize(&self, u16, u16)`、`stop(&self)`、`clear_quota_health(&self)`、`async screen_text(&self, usize) -> Result<String, Closed>`、`async snapshot(&self) -> Result<Snapshot, Closed>`、`async status(&self) -> Result<SessionStatus, Closed>`、`is_closed(&self) -> bool`
  - `async fn start_claude(ClaudeStart, DriverConfig) -> Result<(SessionHandle, mpsc::UnboundedReceiver<SessionEvent>), StartError>`

- [ ] **Step 1: 写失败的测试**

`crates/exec/src/lib.rs` 整个换成：

```rust
//! crewd 的执行器：起 agent 进程（PTY）、读它的屏幕、投递正文、认健康状态、收尸与恢复。
//! 其余部分只经 `driver::SessionHandle` 这一个窄接口与它交互（spec §13 Q12）。
pub mod delivery;
pub mod driver;
pub mod env;
pub mod health;
pub mod launch;
pub mod pty;
pub mod registry;
pub mod resume;
pub mod session;
```

`crates/exec/tests/driver.rs`（整个文件）。「claude」用 shell 脚本顶替：脚本把收到的参数打到屏上，测试用 `screen_text` 读回来核 argv。投递、对话框这些要像样 TUI 的留给 Task 11。`stop_escalates_to_killing_the_process_group` 里孙进程连 HUP 一起忽略——否则会话首进程一退内核就发 SIGHUP 把它带走，分不出 killpg 有没有做。Review Focus 4 的测试在最后：

```rust
//! 驱动层：真 PTY、真进程，「claude」用 shell 脚本顶替（只验启动、账本、停止、续跑被拒重起、
//! 登记表、认证探测；投递与对话框见 `fake_claude.rs`）。
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crewd_exec::driver::{ClaudeStart, DriverConfig, SessionHandle, StartError, start_claude};
use crewd_exec::health::{HealthKind, LaunchVerdict};
use crewd_exec::launch::ClaudeSpec;
use crewd_exec::registry::ProcessRegistry;
use crewd_exec::resume::{AgentIdLedger, MemoryLedger};
use crewd_exec::session::{ExitInfo, ExitStatusKind, SessionEvent};
use tokio::sync::mpsc::UnboundedReceiver;

struct Env {
    dir: tempfile::TempDir,
}

impl Env {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn script(&self, body: &str) -> PathBuf {
        let path = self.dir.path().join("claude");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn config(&self) -> DriverConfig {
        let mut c = DriverConfig::new(self.dir.path().join("run"));
        c.stop_grace = Duration::from_millis(500);
        c
    }

    fn start(&self, program: PathBuf, ledger: Arc<MemoryLedger>) -> ClaudeStart {
        ClaudeStart {
            session_id: "worker-1".into(),
            program,
            child_path: "/usr/bin:/bin".into(),
            spec: ClaudeSpec {
                workdir: self.dir.path().to_path_buf(),
                ..ClaudeSpec::default()
            },
            ledger,
        }
    }
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

async fn screen_contains(h: &SessionHandle, needle: &str) -> String {
    for _ in 0..100 {
        let text = h.screen_text(100).await.unwrap();
        if text.contains(needle) {
            return text;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!(
        "屏上 5s 内没出现 {needle:?}：{}",
        h.screen_text(100).await.unwrap()
    );
}

fn exited(e: &SessionEvent) -> Option<&ExitInfo> {
    match e {
        SessionEvent::Exited(info) => Some(info),
        _ => None,
    }
}

#[tokio::test]
async fn fresh_start_records_the_id_before_spawning_and_passes_it() {
    let env = Env::new();
    let ledger = Arc::new(MemoryLedger::default());
    let program = env.script("echo \"ARGS $*\"; sleep 30");
    let (h, mut rx) = start_claude(env.start(program, ledger.clone()), env.config())
        .await
        .unwrap();
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
    let text = screen_contains(&h, "ARGS").await;
    assert!(
        text.contains(&format!(
            "--session-id {agent_session_id} --permission-mode auto"
        )),
        "{text}"
    );
    h.stop().unwrap();
    let events = until(&mut rx, |e| exited(e).is_some()).await;
    assert_eq!(
        exited(events.last().unwrap()),
        Some(&ExitInfo {
            status: ExitStatusKind::Signal(15),
            stopped_by_user: true,
            hit_limit: false,
        })
    );
}

#[tokio::test]
async fn recorded_id_is_resumed() {
    let env = Env::new();
    let ledger = Arc::new(MemoryLedger::with(Some("old-id")));
    let program = env.script("echo \"ARGS $*\"; sleep 30");
    let (h, mut rx) = start_claude(env.start(program, ledger), env.config())
        .await
        .unwrap();
    assert!(matches!(
        next(&mut rx).await,
        SessionEvent::Spawned { resumed: true, ref agent_session_id, .. } if agent_session_id == "old-id"
    ));
    screen_contains(&h, "ARGS --resume old-id").await;
    h.stop().unwrap();
    until(&mut rx, |e| exited(e).is_some()).await;
}

#[tokio::test]
async fn refused_resume_restarts_once_with_a_fresh_recorded_id() {
    let env = Env::new();
    let ledger = Arc::new(MemoryLedger::with(Some("old-id")));
    let program = env.script(
        "case \"$*\" in\n\
         *--resume*) echo \"No conversation found with session ID: old-id\"; exit 1;;\n\
         *) echo \"FRESH $*\"; sleep 30;;\n\
         esac",
    );
    let (h, mut rx) = start_claude(env.start(program, ledger.clone()), env.config())
        .await
        .unwrap();
    let events = until(&mut rx, |e| {
        matches!(e, SessionEvent::Spawned { resumed: false, .. })
    })
    .await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, SessionEvent::ResumeRefused { id, said }
            if id == "old-id" && said.contains("No conversation found"))),
        "{events:?}"
    );
    assert!(
        !events.iter().any(|e| exited(e).is_some()),
        "被拒的那次不算退出：{events:?}"
    );
    let SessionEvent::Spawned {
        agent_session_id, ..
    } = events.last().unwrap()
    else {
        unreachable!()
    };
    assert_ne!(agent_session_id, "old-id");
    assert_eq!(
        ledger.current().as_deref(),
        Some(agent_session_id.as_str()),
        "新号先落账"
    );
    screen_contains(&h, "FRESH --session-id").await;
    h.stop().unwrap();
    until(&mut rx, |e| exited(e).is_some()).await;
}

#[tokio::test]
async fn stop_escalates_to_killing_the_process_group() {
    let env = Env::new();
    // 孙进程连 HUP 一起忽略：只靠会话首进程退出时内核发的 SIGHUP 收不掉它，必须 killpg。
    let program = env.script("trap '' TERM HUP; sleep 30 & echo \"CHILD $!\"; wait");
    let (h, mut rx) = start_claude(
        env.start(program, Arc::new(MemoryLedger::default())),
        env.config(),
    )
    .await
    .unwrap();
    let text = screen_contains(&h, "CHILD ").await;
    let grandchild: i32 = text
        .split("CHILD ")
        .nth(1)
        .and_then(|s| s.split_whitespace().next())
        .and_then(|s| s.parse().ok())
        .expect("没读到孙进程 pid");
    h.stop().unwrap();
    let events = until(&mut rx, |e| exited(e).is_some()).await;
    assert_eq!(
        exited(events.last().unwrap()).map(|i| i.status.clone()),
        Some(ExitStatusKind::Signal(9)),
        "忽略 SIGTERM 的进程要在 grace 后被 SIGKILL"
    );
    let gone =
        rustix::process::test_kill_process(rustix::process::Pid::from_raw(grandchild).unwrap());
    assert!(gone.is_err(), "同组的孙进程也要一起收掉");
}

#[tokio::test]
async fn missing_program_is_a_start_error_with_launch_health() {
    let env = Env::new();
    let err = start_claude(
        env.start(
            env.dir.path().join("nope"),
            Arc::new(MemoryLedger::default()),
        ),
        env.config(),
    )
    .await
    .err()
    .expect("起不来必须报错");
    match err {
        StartError::Spawn { health, .. } => {
            assert_eq!(health.kind, HealthKind::LaunchFailed);
            assert_eq!(health.launch_verdict, Some(LaunchVerdict::SpawnFailed));
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn dying_without_output_is_reported_as_launch_failure() {
    let env = Env::new();
    let program = env.script("exit 7");
    let (_h, mut rx) = start_claude(
        env.start(program, Arc::new(MemoryLedger::default())),
        env.config(),
    )
    .await
    .unwrap();
    let events = until(&mut rx, |e| exited(e).is_some()).await;
    assert!(
        events.iter().any(|e| matches!(e, SessionEvent::Health(Some(h))
            if h.launch_verdict == Some(LaunchVerdict::DiedSilently) && h.detail.contains("exit 7"))),
        "{events:?}"
    );
}

#[tokio::test]
async fn registry_holds_the_live_process_and_is_cleared_on_exit() {
    let env = Env::new();
    let registry = Arc::new(ProcessRegistry::load(&env.dir.path().join("registry.json")).unwrap());
    let mut config = env.config();
    config.registry = Some(registry.clone());
    let program = env.script("echo up; sleep 30");
    let (h, mut rx) = start_claude(
        env.start(program, Arc::new(MemoryLedger::default())),
        config,
    )
    .await
    .unwrap();
    let SessionEvent::Spawned { pid, .. } = next(&mut rx).await else {
        panic!()
    };
    let entries = registry.entries();
    assert_eq!(entries.len(), 1);
    assert_eq!(
        (entries[0].0.as_str(), entries[0].1.pid),
        ("worker-1", pid as i32)
    );
    h.stop().unwrap();
    until(&mut rx, |e| exited(e).is_some()).await;
    // 任务在发出 Exited 之后才清账，给它一拍。
    for _ in 0..50 {
        if registry.entries().is_empty() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("退出后登记表没清：{:?}", registry.entries());
}

fn write_auth_failure(projects: &Path, id: &str) {
    let dir = projects.join("-tmp-project");
    std::fs::create_dir_all(&dir).unwrap();
    let ts = chrono::DateTime::<chrono::Utc>::from(std::time::SystemTime::now())
        + chrono::Duration::seconds(1);
    let record = serde_json::json!({
        "sessionId": id, "type": "assistant", "isSidechain": false,
        "timestamp": ts.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        "isApiErrorMessage": true, "error": "authentication_failed",
        "message": {"role": "assistant", "id": "msg_x", "model": "m", "content": []},
    });
    std::fs::write(dir.join(format!("{id}.jsonl")), format!("{record}\n")).unwrap();
}

#[tokio::test]
async fn auth_failure_in_the_transcript_raises_health() {
    let env = Env::new();
    let projects = env.dir.path().join("projects");
    let mut config = env.config();
    config.claude_projects_dir = Some(projects.clone());
    config.auth_poll = Duration::from_millis(100);
    let program = env.script("echo up; sleep 30");
    let (h, mut rx) = start_claude(
        env.start(program, Arc::new(MemoryLedger::default())),
        config,
    )
    .await
    .unwrap();
    let SessionEvent::Spawned {
        agent_session_id, ..
    } = next(&mut rx).await
    else {
        panic!()
    };
    write_auth_failure(&projects, &agent_session_id);
    until(
        &mut rx,
        |e| matches!(e, SessionEvent::Health(Some(h)) if h.kind == HealthKind::AuthRequired),
    )
    .await;
    h.stop().unwrap();
    until(&mut rx, |e| exited(e).is_some()).await;
}

#[tokio::test]
async fn raw_input_and_resize_reach_the_child() {
    let env = Env::new();
    let program = env.script("stty size; read l; echo \"got:$l\"; stty size; sleep 30");
    let (h, mut rx) = start_claude(
        env.start(program, Arc::new(MemoryLedger::default())),
        env.config(),
    )
    .await
    .unwrap();
    screen_contains(&h, "25 80").await;
    h.resize(100, 30).unwrap();
    h.raw(b"hi\r".to_vec()).unwrap();
    screen_contains(&h, "got:hi").await;
    screen_contains(&h, "30 100").await;
    let status = h.status().await.unwrap();
    assert_eq!(status.agent_session_id.len(), 36);
    h.stop().unwrap();
    until(&mut rx, |e| exited(e).is_some()).await;
}

#[tokio::test]
async fn dropping_every_handle_stops_the_session() {
    let env = Env::new();
    let program = env.script("echo up; sleep 30");
    let (h, mut rx) = start_claude(
        env.start(program, Arc::new(MemoryLedger::default())),
        env.config(),
    )
    .await
    .unwrap();
    let h2 = h.clone();
    drop(h);
    drop(h2);
    let events = until(&mut rx, |e| exited(e).is_some()).await;
    assert!(exited(events.last().unwrap()).unwrap().stopped_by_user);
}

/// 账本写不进去（session 行没了 / 库坏了）。
struct BrokenLedger;

impl AgentIdLedger for BrokenLedger {
    fn recorded(&self) -> std::io::Result<Option<String>> {
        Ok(None)
    }

    fn record(&self, _: &str) -> std::io::Result<()> {
        Err(std::io::Error::other("库坏了"))
    }
}

/// Review Focus 4：账本写不进去就绝不起进程——起了也接不回来，下次重启又是新对话。
#[tokio::test]
async fn ledger_failure_means_no_process() {
    let env = Env::new();
    let marker = env.dir.path().join("ran");
    let program = env.script(&format!("touch '{}'; sleep 30", marker.display()));
    let start = ClaudeStart {
        ledger: Arc::new(BrokenLedger),
        ..env.start(program, Arc::new(MemoryLedger::default()))
    };
    let err = start_claude(start, env.config())
        .await
        .err()
        .expect("必须报错");
    assert!(matches!(err, StartError::Ledger(_)), "{err:?}");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!marker.exists(), "账本失败后进程不该被起过");
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p crewd-exec --test driver`
Expected: 编译失败，``file not found for module `driver` ``（lib.rs 声明了还没写的模块）。

- [ ] **Step 3: 实现**

`crates/exec/src/driver.rs`（整个文件；末尾是只在 Linux 编译的「退出后读干净」单测，理由见本任务开头）：

```rust
//! 一个 claude 会话的 tokio 任务，和其余部分唯一的接口 [`SessionHandle`]（spec §13 Q12）。
//!
//! 任务拥有 PTY 主端、子进程、[`Core`]；命令从句柄进，事件从 `mpsc` 出。
//! 节拍：每块 PTY 输出一拍、0.6s 定时一拍、`Core::next_wakeup` 到点一拍。
//! 写 PTY 走单独的写任务：子进程不读输入时写会阻塞，不能卡住读（否则双方互等）。
//!
//! 以后执行器拆成独立进程，只把 `SessionHandle` 的命令与事件换成线协议。
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime};

use crewd_term::{PendingDecision, ScreenConfig, Snapshot};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::delivery::Subject;
use crate::env::build_env_from_process;
use crate::health::{
    AUTH_POLL_INTERVAL, AuthProbe, AuthState, Health, HealthKind, LaunchVerdict, launch_failure,
};
use crate::launch::{
    AgentSessionArg, ClaudeSpec, LaunchFiles, new_agent_session_id, prepare_claude,
};
use crate::pty::{PtyMaster, SpawnSpec, spawn_pty};
use crate::registry::{ProbeResult, ProcessRegistry, probe};
use crate::resume::{AgentIdLedger, decide_resume};
use crate::session::{
    Action, BRIEF_DELIVERY_ID, Core, CoreConfig, DerivedState, ExitStatusKind, PendingText,
    SessionEvent,
};

/// 驱动层参数。
#[derive(Clone, Debug)]
pub struct DriverConfig {
    pub core: CoreConfig,
    pub screen: ScreenConfig,
    /// 定时节拍。
    pub tick: Duration,
    /// SIGTERM 之后多久升级成 `killpg(SIGKILL)`。
    pub stop_grace: Duration,
    /// 启动临时文件放哪（建议 `<数据目录>/run`，数据目录本身是 0700）。
    pub runtime_dir: PathBuf,
    /// claude 的 projects 目录（认证探测用）；`None` = 不探测。
    pub claude_projects_dir: Option<PathBuf>,
    pub auth_poll: Duration,
    /// 孤儿登记表；`None` = 不登记。
    pub registry: Option<Arc<ProcessRegistry>>,
}

impl DriverConfig {
    pub fn new(runtime_dir: PathBuf) -> Self {
        Self {
            core: CoreConfig::default(),
            screen: ScreenConfig::default(),
            tick: Duration::from_millis(600),
            stop_grace: Duration::from_secs(2),
            runtime_dir,
            claude_projects_dir: None,
            auth_poll: AUTH_POLL_INTERVAL,
            registry: None,
        }
    }
}

/// 起一个 claude 会话要的东西。
pub struct ClaudeStart {
    /// crewd 的 session id（登记表的键）。
    pub session_id: String,
    /// claude 可执行文件（`AgentPaths::resolve("claude")`）。
    pub program: PathBuf,
    /// 子进程 PATH（`AgentPaths::child_path`）。
    pub child_path: String,
    pub spec: ClaudeSpec,
    pub ledger: Arc<dyn AgentIdLedger>,
}

#[derive(Debug)]
pub enum StartError {
    /// 账本读写失败：不起进程（起了也接不回来）。
    Ledger(io::Error),
    /// 临时文件写不出来。
    Files(io::Error),
    /// 进程没起来。`health` 是给人看的拉起失败。
    Spawn { health: Health, source: io::Error },
    /// 登记表写不进去（孤儿兜底失效）：已杀掉刚起的进程。
    Registry(io::Error),
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ledger(e) => write!(f, "会话号账本读写失败：{e}"),
            Self::Files(e) => write!(f, "启动文件写不出来：{e}"),
            Self::Spawn { health, .. } => write!(f, "{}", health.detail),
            Self::Registry(e) => write!(f, "进程登记表写不进去：{e}"),
        }
    }
}

impl std::error::Error for StartError {}

/// 会话任务已经结束。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Closed;

/// 某一时刻的会话状态。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionStatus {
    pub state: DerivedState,
    pub working: bool,
    pub health: Option<Health>,
    pub pending_decision: Option<PendingDecision>,
    pub pid: u32,
    pub agent_session_id: String,
}

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

    pub fn resize(&self, cols: u16, lines: u16) -> Result<(), Closed> {
        self.cmd(Command::Resize { cols, lines })
    }

    /// SIGTERM，`stop_grace` 后 `killpg(SIGKILL)`。退出后发 `Exited { stopped_by_user: true }`。
    pub fn stop(&self) -> Result<(), Closed> {
        self.cmd(Command::Stop)
    }

    /// 额度重置唤醒到点后调：清额度类 health、重新武装扫描器。
    pub fn clear_quota_health(&self) -> Result<(), Closed> {
        self.cmd(Command::ClearQuotaHealth)
    }

    pub async fn screen_text(&self, max_lines: usize) -> Result<String, Closed> {
        let (reply, rx) = oneshot::channel();
        self.cmd(Command::ScreenText { max_lines, reply })?;
        rx.await.map_err(|_| Closed)
    }

    pub async fn snapshot(&self) -> Result<Snapshot, Closed> {
        let (reply, rx) = oneshot::channel();
        self.cmd(Command::Snapshot { reply })?;
        rx.await.map_err(|_| Closed)
    }

    pub async fn status(&self) -> Result<SessionStatus, Closed> {
        let (reply, rx) = oneshot::channel();
        self.cmd(Command::Status { reply })?;
        rx.await.map_err(|_| Closed)
    }

    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }

    fn cmd(&self, c: Command) -> Result<(), Closed> {
        self.tx.send(c).map_err(|_| Closed)
    }
}

/// 起一个 claude 会话。账本：记过就 `--resume`，否则新号且**起进程前**落账。
/// 返回句柄和事件流（第一条是 `Spawned`）。
pub async fn start_claude(
    start: ClaudeStart,
    config: DriverConfig,
) -> Result<(SessionHandle, mpsc::UnboundedReceiver<SessionEvent>), StartError> {
    let ledger = start.ledger.clone();
    let recorded = tokio::task::spawn_blocking(move || ledger.recorded())
        .await
        .map_err(|e| StartError::Ledger(io::Error::other(e)))?
        .map_err(StartError::Ledger)?;
    let arg = decide_resume(recorded.as_deref());
    let (auth_tx, auth_rx) = mpsc::unbounded_channel();
    let running = launch(&start, &config, arg, &auth_tx).await?;
    let (ev_tx, ev_rx) = mpsc::unbounded_channel();
    let (tx, cmd_rx) = mpsc::unbounded_channel();
    let pending = start
        .spec
        .initial_prompt
        .iter()
        .filter(|p| !p.is_empty())
        .map(|text| PendingText {
            id: BRIEF_DELIVERY_ID,
            subject: Subject::Brief,
            text: text.clone(),
        })
        .collect();
    let resumed = match &running.agent {
        AgentSessionArg::Resume(id) => Some(id.clone()),
        AgentSessionArg::New(_) => None,
    };
    let core = Core::new(config.core, config.screen, Instant::now(), pending, resumed);
    let _ = ev_tx.send(running.spawned_event());
    tokio::spawn(run(Task {
        start,
        config,
        core,
        running,
        cmd_rx,
        ev_tx,
        auth_tx,
        auth_rx,
    }));
    Ok((
        SessionHandle {
            tx,
            next_id: Arc::new(AtomicU64::new(BRIEF_DELIVERY_ID + 1)),
        },
        ev_rx,
    ))
}

/// 一个起好的进程及其附属物。
struct Running {
    child: tokio::process::Child,
    pid: u32,
    master: Arc<PtyMaster>,
    writer: mpsc::UnboundedSender<Vec<u8>>,
    writer_task: JoinHandle<()>,
    auth_task: Option<JoinHandle<()>>,
    agent: AgentSessionArg,
    _files: LaunchFiles,
}

impl Running {
    fn spawned_event(&self) -> SessionEvent {
        SessionEvent::Spawned {
            pid: self.pid,
            agent_session_id: self.agent.id().to_string(),
            resumed: matches!(self.agent, AgentSessionArg::Resume(_)),
        }
    }

    fn shutdown_helpers(&mut self) {
        if let Some(t) = self.auth_task.take() {
            t.abort();
        }
        self.writer_task.abort();
    }
}

type AuthMsg = (String, AuthState);

async fn launch(
    start: &ClaudeStart,
    config: &DriverConfig,
    agent: AgentSessionArg,
    auth_tx: &mpsc::UnboundedSender<AuthMsg>,
) -> Result<Running, StartError> {
    if let AgentSessionArg::New(id) = &agent {
        let (ledger, id) = (start.ledger.clone(), id.clone());
        tokio::task::spawn_blocking(move || ledger.record(&id))
            .await
            .map_err(|e| StartError::Ledger(io::Error::other(e)))?
            .map_err(StartError::Ledger)?;
    }
    let (args, files) = prepare_claude(&start.spec, agent.clone(), &config.runtime_dir)
        .map_err(StartError::Files)?;
    let env = build_env_from_process(&start.child_path, &start.spec.additional_env);
    let since = SystemTime::now();
    let spec = SpawnSpec {
        program: start.program.clone(),
        args: args.argv(),
        env,
        cwd: start.spec.workdir.clone(),
        cols: config.screen.cols as u16,
        lines: config.screen.lines as u16,
    };
    let proc = spawn_pty(&spec).map_err(|source| {
        let underlying = format!("{}: {source}", start.program.display());
        let health = launch_failure(
            LaunchVerdict::SpawnFailed,
            config.core.launch_deadline,
            Some(&underlying),
        )
        .expect("SpawnFailed 一定有说明");
        StartError::Spawn { health, source }
    })?;
    let master = Arc::new(proc.master);
    let mut child = proc.child;
    if let Some(registry) = &config.registry {
        let recorded = match probe(proc.pid as i32) {
            ProbeResult::Found(identity) => registry.record(&start.session_id, identity),
            other => Err(io::Error::other(format!("读不到刚起的进程身份：{other:?}"))),
        };
        if let Err(e) = recorded {
            kill_group(proc.pid);
            let _ = child.wait().await;
            return Err(StartError::Registry(e));
        }
    }
    let (writer, mut writes) = mpsc::unbounded_channel::<Vec<u8>>();
    let w = master.clone();
    let writer_task = tokio::spawn(async move {
        while let Some(bytes) = writes.recv().await {
            if w.write_all(&bytes).await.is_err() {
                break;
            }
        }
    });
    let auth_task = config.claude_projects_dir.clone().map(|projects_dir| {
        let probe = AuthProbe {
            session_id: agent.id().to_string(),
            projects_dir,
            since,
        };
        let (tx, every) = (auth_tx.clone(), config.auth_poll);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(every).await;
                let p = probe.clone();
                let Ok(state) = tokio::task::spawn_blocking(move || p.read()).await else {
                    return;
                };
                if tx.send((probe.session_id.clone(), state)).is_err() {
                    return;
                }
            }
        })
    });
    Ok(Running {
        child,
        pid: proc.pid,
        master,
        writer,
        writer_task,
        auth_task,
        agent,
        _files: files,
    })
}

struct Task {
    start: ClaudeStart,
    config: DriverConfig,
    core: Core,
    running: Running,
    cmd_rx: mpsc::UnboundedReceiver<Command>,
    ev_tx: mpsc::UnboundedSender<SessionEvent>,
    auth_tx: mpsc::UnboundedSender<AuthMsg>,
    auth_rx: mpsc::UnboundedReceiver<AuthMsg>,
}

fn kill_group(pid: u32) {
    use rustix::process::{Pid, Signal, kill_process, kill_process_group};
    if let Some(p) = Pid::from_raw(pid as i32)
        && kill_process_group(p, Signal::KILL).is_err()
    {
        let _ = kill_process(p, Signal::KILL);
    }
}

fn terminate(pid: u32) {
    use rustix::process::{Pid, Signal, kill_process};
    if let Some(p) = Pid::from_raw(pid as i32) {
        let _ = kill_process(p, Signal::TERM);
    }
}

fn decode(status: io::Result<std::process::ExitStatus>) -> ExitStatusKind {
    use std::os::unix::process::ExitStatusExt;
    match status {
        Ok(s) => match (s.code(), s.signal()) {
            (Some(code), _) => ExitStatusKind::Code(code),
            (None, Some(sig)) => ExitStatusKind::Signal(sig),
            (None, None) => ExitStatusKind::Code(-1),
        },
        Err(_) => ExitStatusKind::Code(-1),
    }
}

/// 子进程退出后把 PTY 里剩下的输出读干净（最多 `limit`）：claude 拒绝续跑时是先打印再退出的。
async fn drain(master: &PtyMaster, core: &mut Core, limit: Duration) {
    let deadline = tokio::time::Instant::now() + limit;
    let mut buf = vec![0u8; 16 * 1024];
    while let Ok(Ok(n)) = tokio::time::timeout_at(deadline, master.read(&mut buf)).await {
        if n == 0 {
            break;
        }
        core.on_output(&buf[..n], Instant::now());
    }
}

async fn run(mut t: Task) {
    let mut buf = vec![0u8; 16 * 1024];
    let mut reading = true;
    let mut commands_open = true;
    let mut stop_deadline: Option<tokio::time::Instant> = None;
    let mut retried = false;
    let mut tick = tokio::time::interval(t.config.tick);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        let wake = t.core.next_wakeup().map(tokio::time::Instant::from_std);
        let mut exit = None;
        tokio::select! {
            r = t.running.master.read(&mut buf), if reading => match r {
                Ok(n) if n > 0 => t.core.on_output(&buf[..n], Instant::now()),
                _ => reading = false,
            },
            status = t.running.child.wait() => exit = Some(decode(status)),
            cmd = t.cmd_rx.recv(), if commands_open => match cmd {
                Some(c) => {
                    if handle(&mut t, c) {
                        stop_deadline = Some(tokio::time::Instant::now() + t.config.stop_grace);
                    }
                }
                None => {
                    commands_open = false;
                    t.core.stop(Instant::now());
                    terminate(t.running.pid);
                    stop_deadline = Some(tokio::time::Instant::now() + t.config.stop_grace);
                }
            },
            _ = tick.tick() => t.core.on_tick(Instant::now()),
            _ = tokio::time::sleep_until(wake.unwrap_or_else(tokio::time::Instant::now)), if wake.is_some() => {
                t.core.on_tick(Instant::now());
            }
            _ = tokio::time::sleep_until(stop_deadline.unwrap_or_else(tokio::time::Instant::now)), if stop_deadline.is_some() => {
                stop_deadline = None;
                kill_group(t.running.pid);
            }
            Some((id, state)) = t.auth_rx.recv() => {
                if id == t.running.agent.id() {
                    t.core.on_auth(state, Instant::now());
                }
            }
        }
        if exit.is_some() {
            drain(&t.running.master, &mut t.core, Duration::from_millis(500)).await;
        }
        let refused = execute(&mut t);
        if refused && !retried {
            retried = true;
            // 被拒的那个进程：已经自己退了就用它的退出状态，还活着就杀掉收尸。
            let old_status = match exit.take() {
                Some(status) => status,
                None => {
                    kill_group(t.running.pid);
                    decode(t.running.child.wait().await)
                }
            };
            match respawn(&mut t).await {
                Ok(()) => {
                    reading = true;
                    continue;
                }
                Err(e) => {
                    let health = match &e {
                        StartError::Spawn { health, .. } => health.clone(),
                        other => Health::new(HealthKind::LaunchFailed, other.to_string()),
                    };
                    let _ = t.ev_tx.send(SessionEvent::Health(Some(health)));
                    t.core.on_exit(old_status, Instant::now());
                    execute(&mut t);
                    break;
                }
            }
        }
        if let Some(status) = exit {
            t.core.on_exit(status, Instant::now());
            execute(&mut t);
            break;
        }
    }
    t.running.shutdown_helpers();
    if let Some(registry) = &t.config.registry {
        let _ = registry.forget(&t.start.session_id);
    }
}

/// 处理一条命令；返回 true = 开始停止（要起 grace 计时）。
fn handle(t: &mut Task, cmd: Command) -> bool {
    let now = Instant::now();
    match cmd {
        Command::Send { id, text } => t.core.send(id, &text, now),
        Command::Raw(bytes) => t.core.raw(&bytes),
        Command::Resize { cols, lines } => t.core.resize(cols, lines),
        Command::Stop => {
            if t.core.is_running() {
                t.core.stop(now);
                terminate(t.running.pid);
                return true;
            }
        }
        Command::ClearQuotaHealth => t.core.clear_quota_health(now),
        Command::ScreenText { max_lines, reply } => {
            let _ = reply.send(t.core.screen_text(max_lines));
        }
        Command::Snapshot { reply } => {
            let _ = reply.send(t.core.snapshot());
        }
        Command::Status { reply } => {
            let _ = reply.send(SessionStatus {
                state: t.core.state(),
                working: t.core.is_working(),
                health: t.core.health().cloned(),
                pending_decision: t.core.pending_decision().cloned(),
                pid: t.running.pid,
                agent_session_id: t.running.agent.id().to_string(),
            });
        }
    }
    false
}

/// 执行 Core 攒下的动作；返回是否见到了 `ResumeRefused`。
fn execute(t: &mut Task) -> bool {
    let mut refused = false;
    for action in t.core.take_actions() {
        match action {
            Action::Write(bytes) => {
                let _ = t.running.writer.send(bytes);
            }
            Action::Resize { cols, lines } => {
                let _ = t.running.master.resize(cols, lines);
            }
            Action::Event(e) => {
                refused |= matches!(e, SessionEvent::ResumeRefused { .. });
                let _ = t.ev_tx.send(e);
            }
        }
    }
    refused
}

/// claude 拒绝了 `--resume`：换新号（先落账）重起一次，没投出去的正文搬过去。
async fn respawn(t: &mut Task) -> Result<(), StartError> {
    let pending = t.core.take_pending();
    let agent = AgentSessionArg::New(new_agent_session_id());
    let running = launch(&t.start, &t.config, agent, &t.auth_tx).await?;
    let mut old = std::mem::replace(&mut t.running, running);
    old.shutdown_helpers();
    t.core = Core::new(
        t.config.core,
        t.config.screen,
        Instant::now(),
        pending,
        None,
    );
    let _ = t.ev_tx.send(t.running.spawned_event());
    Ok(())
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// claude 拒绝续跑是「打印一行就退出」：退出被先看到时，PTY 里还躺着那行字。
    /// 只在 Linux 上跑：macOS 关闭 tty 从端时会等输出被读走（`ttywait`），没人读主端时
    /// 子进程根本退不掉——那边「先看到退出、后读输出」不会发生（实测：这条在 macOS 上挂住）。
    #[tokio::test]
    async fn output_left_in_the_pty_after_exit_is_drained() {
        let mut env = BTreeMap::new();
        env.insert("PATH".to_string(), "/usr/bin:/bin".to_string());
        let mut p = spawn_pty(&SpawnSpec {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "echo last-words; exit 1".into()],
            env,
            cwd: std::env::temp_dir(),
            cols: 80,
            lines: 25,
        })
        .unwrap();
        p.child.wait().await.unwrap();
        let mut core = Core::new(
            CoreConfig::default(),
            ScreenConfig::default(),
            Instant::now(),
            vec![],
            None,
        );
        drain(&p.master, &mut core, Duration::from_millis(500)).await;
        assert!(core.screen_text(25).contains("last-words"));
    }
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p crewd-exec --test driver`（连跑三遍，看有没有时序抖动）
Expected: `11 passed`，每遍 1s 左右。`cargo test -p crewd-exec --lib` 仍是 `81 passed`（driver 的单测在 macOS 上不编译）。

- [ ] **Step 5: 验这几把尺子会红**

每刀单独做、做完改回：
1. `kill_group` 里 `&& kill_process_group(p, Signal::KILL).is_err()` 临时改成 `&& kill_process(p, Signal::KILL).is_err()` → `cargo test -p crewd-exec --test driver` → 红 1 条：`stop_escalates_to_killing_the_process_group`。这一刀会留下一个忽略 TERM/HUP 的 `sleep 30`，30 秒后自己退出，不用管。
2. `launch` 里 `tokio::task::spawn_blocking(move || ledger.record(&id))` 临时改成 `tokio::task::spawn_blocking(move || { let _ = (ledger, id); Ok::<(), io::Error>(()) })` → 红 3 条：`fresh_start_records_the_id_before_spawning_and_passes_it`、`refused_resume_restarts_once_with_a_fresh_recorded_id`、`ledger_failure_means_no_process`。

- [ ] **Step 6: 全量检查并提交**

Run: `./scripts/check.sh` → 0。

```bash
git add crates/exec/src/lib.rs crates/exec/src/driver.rs crates/exec/tests/driver.rs
git commit -m "crewd-exec: session driver task and the narrow SessionHandle interface

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 11: 测试器材——假 claude、端到端、真机冒烟

假 claude（`src/bin/fake_claude.rs`，cargo 会给集成测试 `CARGO_BIN_EXE_fake_claude`）是一个只会画几屏的小程序，跑在真 PTY 里、切到 raw 模式：输入框（空框显示 `Try "…"` 提示，多行或超长输入折成 `[Pasted text #1 +N lines]`，跟真 claude 一样让「落地」判据不能逐字比）、信任框（可以先原样回放 M2a 录下来的 `tui-claude-trust.bin`，再响应 `↓` + 回车）、限流菜单、额度报错退出、永不回显、永不读输入。**它照真 claude 模拟了粘贴陷阱**：同一次 read 里收到多个字节 = 粘贴，其中的 CR 当换行；粘贴之后单独来的回车只结束粘贴，要先来一个普通按键才提交（Swift 在真 daemon 上看到的行为，也是三拍提交的由来）。Swift 版的假 TUI 没模拟这个，它自己的注释承认那几条测试挡不住「正文 + 回车一笔发出」的旧实现；这里有 `fake_models_the_paste_trap` 先自检器材，再有变异步骤证明三拍是被测到的。

假 claude 的行为由环境变量控制；执行器只放行调用方显式给的变量，所以测试经 `ClaudeSpec::additional_env` 传（Swift 版第一稿就栽在这：测试进程 `setenv` 的东西根本传不到子进程，四条全绿得莫名其妙）。

真机冒烟只验「起得来、终端查询答得对、画出输入框」，**不发任何 prompt**（不花额度、不生成会话日志），两道闸（Global Constraints）。它就是 spec §15 要的「DA1 回答串与 SwiftTerm 不同，M2b 接上 PTY 后真机核一次」。

**Files:**
- Create: `crewd/crates/exec/src/bin/fake_claude.rs`、`crewd/crates/exec/tests/fake_claude.rs`、`crewd/crates/exec/tests/live_claude.rs`

**Interfaces:**
- Consumes: Task 10 的窄接口；Task 6 `session::{BRIEF_DELIVERY_ID, DeliveryOutcome, ExitStatusKind, SessionEvent}`；Task 1 `env::AgentPaths`；M2a `crewd_term::{DecisionEvent, blocking_dialog, input_row}`。
- Produces: 二进制 `fake_claude`（只给测试用；环境变量 `FAKE_CLAUDE_MODE` = `ready` / `deaf` / `noread` / `trust` / `ratelimit` / `usage`，`FAKE_CLAUDE_READY_DELAY_MS`、`FAKE_CLAUDE_REPLAY`、`FAKE_CLAUDE_LOG`）。

- [ ] **Step 1: 写失败的测试**

`crates/exec/tests/fake_claude.rs`（整个文件）。所有用例都把 `verify_window` 缩到 0.8s、`submit_gap` 缩到 0.3s；信任框那条里投递机的 Blocked（3s）和待决策跟踪器的 Appeared（3s、挂在 0.6s 节拍上）谁先到不一定，测试两个都等。Review Focus 2 / 5 的测试在最后：

```rust
//! 端到端：真 PTY + 假 claude（`src/bin/fake_claude.rs`）。投递、粘贴陷阱、信任框（回放
//! M2a 的真录制字节）、限流菜单自动应答、额度用尽退出。不碰真 claude（真机见 `live_claude.rs`）。
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crewd_exec::driver::{ClaudeStart, DriverConfig, SessionHandle, start_claude};
use crewd_exec::health::HealthKind;
use crewd_exec::launch::ClaudeSpec;
use crewd_exec::resume::MemoryLedger;
use crewd_exec::session::{BRIEF_DELIVERY_ID, DeliveryOutcome, ExitStatusKind, SessionEvent};
use crewd_term::DecisionEvent;
use tokio::sync::mpsc::UnboundedReceiver;

const FAKE: &str = env!("CARGO_BIN_EXE_fake_claude");

fn trust_fixture() -> String {
    let path = format!(
        "{}/../term/tests/fixtures/tui-claude-trust.bin",
        env!("CARGO_MANIFEST_DIR")
    );
    assert!(std::path::Path::new(&path).exists(), "缺夹具 {path}");
    path
}

struct Run {
    h: SessionHandle,
    rx: UnboundedReceiver<SessionEvent>,
    log: PathBuf,
    _dir: tempfile::TempDir,
}

async fn start(fake_env: &[(&str, &str)], brief: Option<&str>) -> Run {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("submitted.log");
    let mut additional_env: BTreeMap<String, String> = fake_env
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    additional_env.insert("FAKE_CLAUDE_LOG".into(), log.display().to_string());
    let mut config = DriverConfig::new(dir.path().join("run"));
    config.stop_grace = Duration::from_millis(500);
    // 缩短重试窗口，「投不进去」那条不用等 7.5s。
    config.core.delivery.verify_window = Duration::from_millis(800);
    config.core.delivery.submit_gap = Duration::from_millis(300);
    let start = ClaudeStart {
        session_id: "worker-1".into(),
        program: PathBuf::from(FAKE),
        child_path: "/usr/bin:/bin".into(),
        spec: ClaudeSpec {
            workdir: dir.path().to_path_buf(),
            initial_prompt: brief.map(str::to_string),
            additional_env,
            ..ClaudeSpec::default()
        },
        ledger: Arc::new(MemoryLedger::default()),
    };
    let (h, rx) = start_claude(start, config).await.unwrap();
    Run {
        h,
        rx,
        log,
        _dir: dir,
    }
}

impl Run {
    async fn until(&mut self, pred: impl Fn(&SessionEvent) -> bool) -> Vec<SessionEvent> {
        let mut seen = Vec::new();
        loop {
            let e = tokio::time::timeout(Duration::from_secs(15), self.rx.recv())
                .await
                .unwrap_or_else(|_| panic!("15s 内没等到；已收到 {seen:?}"))
                .expect("事件流提前结束");
            let hit = pred(&e);
            seen.push(e);
            if hit {
                return seen;
            }
        }
    }

    fn submitted(&self) -> Vec<String> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    async fn stop(mut self) {
        let _ = self.h.stop();
        self.until(|e| matches!(e, SessionEvent::Exited(_))).await;
    }
}

fn outcome(id: u64, pred: impl Fn(&DeliveryOutcome) -> bool) -> impl Fn(&SessionEvent) -> bool {
    move |e| matches!(e, SessionEvent::Delivery { id: i, outcome, .. } if *i == id && pred(outcome))
}

#[tokio::test]
async fn brief_waits_out_a_slow_first_screen_then_is_submitted_once() {
    let mut r = start(
        &[("FAKE_CLAUDE_READY_DELAY_MS", "1500")],
        Some("开场任务：只回答 OK"),
    )
    .await;
    r.until(outcome(BRIEF_DELIVERY_ID, |o| {
        *o == DeliveryOutcome::Delivered
    }))
    .await;
    assert_eq!(
        r.submitted(),
        ["开场任务：只回答 OK"],
        "只提交一次、一字不差"
    );
    assert!(r.h.screen_text(25).await.unwrap().contains("GOT: 开场任务"));
    r.stop().await;
}

#[tokio::test]
async fn long_brief_folded_into_a_paste_placeholder_still_lands() {
    let brief: String = (1..=40).map(|i| format!("第 {i} 行说明\n")).collect();
    let mut r = start(&[], Some(&brief)).await;
    r.until(outcome(BRIEF_DELIVERY_ID, |o| {
        *o == DeliveryOutcome::Delivered
    }))
    .await;
    assert_eq!(r.submitted(), [brief.replace('\n', "\\n")]);
    r.stop().await;
}

#[tokio::test]
async fn steers_are_queued_and_each_submitted_once_in_order() {
    let mut r = start(&[], None).await;
    let a = r.h.send("第一条").unwrap();
    let b = r.h.send("第二条").unwrap();
    r.until(outcome(a, |o| *o == DeliveryOutcome::Delivered))
        .await;
    r.until(outcome(b, |o| *o == DeliveryOutcome::Delivered))
        .await;
    assert_eq!(r.submitted(), ["第一条", "第二条"]);
    r.stop().await;
}

/// 器材自检：假 claude 真的模拟了粘贴陷阱——正文和回车同一笔到达，回车被当成换行；
/// 紧跟着单独发回车也只结束粘贴；先来一个普通按键才提交。没有这一条，上面几条绿了
/// 也证明不了「空格 + 退格 + 回车」三拍有必要。
#[tokio::test]
async fn fake_models_the_paste_trap() {
    let mut r = start(&[], None).await;
    r.until(|e| matches!(e, SessionEvent::Working(false))).await;
    let settle = || tokio::time::sleep(Duration::from_millis(300));
    r.h.raw(b"hello\r".to_vec()).unwrap();
    settle().await;
    r.h.raw(b"\r".to_vec()).unwrap();
    settle().await;
    assert!(
        r.submitted().is_empty(),
        "粘贴里的 CR、紧跟的单独回车都不该提交"
    );
    for key in [&b" "[..], &[0x7f], b"\r"] {
        r.h.raw(key.to_vec()).unwrap();
        settle().await;
    }
    assert_eq!(r.submitted(), ["hello\\n"], "普通按键之后的回车才提交");
    r.stop().await;
}

#[tokio::test]
async fn deaf_input_box_is_reported_undelivered() {
    let mut r = start(&[("FAKE_CLAUDE_MODE", "deaf")], Some("没人听")).await;
    let seen = r
        .until(outcome(BRIEF_DELIVERY_ID, |o| {
            matches!(o, DeliveryOutcome::Undelivered(_))
        }))
        .await;
    assert!(seen.iter().any(
        |e| matches!(e, SessionEvent::Health(Some(h)) if h.kind == HealthKind::BriefUndelivered)
    ));
    r.stop().await;
}

/// 回放真 claude 录下来的信任框首屏：不投、报待决策（无编号）与 Blocked；有人按「↓ 回车」代答后自己补投。
#[tokio::test]
async fn recorded_trust_dialog_blocks_until_answered_then_brief_lands() {
    let fixture = trust_fixture();
    let mut r = start(
        &[
            ("FAKE_CLAUDE_MODE", "trust"),
            ("FAKE_CLAUDE_REPLAY", fixture.as_str()),
        ],
        Some("信任之后再干活"),
    )
    .await;
    // Blocked（投递机 3s）与 Appeared（待决策跟踪器 3s，挂在 0.6s 节拍上）谁先到不一定。
    let blocked = outcome(BRIEF_DELIVERY_ID, |o| {
        matches!(o, DeliveryOutcome::Blocked(_))
    });
    let appeared = |e: &SessionEvent| matches!(e, SessionEvent::Decision(DecisionEvent::Appeared(d)) if !d.numbered);
    let mut seen = r.until(|e| blocked(e) || appeared(e)).await;
    if !seen.iter().any(&blocked) {
        seen.extend(r.until(&blocked).await);
    }
    if !seen.iter().any(appeared) {
        seen.extend(r.until(appeared).await);
    }
    assert!(r.submitted().is_empty());
    r.h.raw(b"\x1b[B".to_vec()).unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    r.h.raw(b"\r".to_vec()).unwrap();
    let seen = r
        .until(outcome(BRIEF_DELIVERY_ID, |o| {
            *o == DeliveryOutcome::Delivered
        }))
        .await;
    assert!(
        seen.contains(&SessionEvent::Decision(DecisionEvent::Cleared)),
        "{seen:?}"
    );
    assert!(
        seen.contains(&SessionEvent::Health(None)),
        "补投成功后清掉 BriefUndelivered"
    );
    assert_eq!(r.submitted(), ["信任之后再干活"]);
    r.stop().await;
}

#[tokio::test]
async fn rate_limit_menu_is_answered_automatically() {
    let mut r = start(&[("FAKE_CLAUDE_MODE", "ratelimit")], None).await;
    r.h.send("触发限流").unwrap();
    r.until(|e| matches!(e, SessionEvent::Health(Some(h)) if h.kind == HealthKind::RateLimited))
        .await;
    for _ in 0..100 {
        if r.submitted().len() == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(r.submitted(), ["触发限流", "RATE-ANSWER:1"]);
    r.stop().await;
}

#[tokio::test]
async fn usage_limit_then_exit_is_marked_hit_limit() {
    let mut r = start(&[("FAKE_CLAUDE_MODE", "usage")], Some("干活")).await;
    let seen = r.until(|e| matches!(e, SessionEvent::Exited(_))).await;
    assert!(
        seen.iter().any(
            |e| matches!(e, SessionEvent::Health(Some(h)) if h.kind == HealthKind::UsageLimit)
        )
    );
    let Some(SessionEvent::Exited(info)) = seen.last() else {
        unreachable!()
    };
    assert_eq!(info.status, ExitStatusKind::Code(1));
    assert!(info.hit_limit && !info.stopped_by_user);
}

/// Review Focus 2：子进程不读输入、我们却要写一大段——写任务卡住，但会话不能跟着卡死：
/// 事件照常、停得掉。
#[tokio::test]
async fn a_child_that_never_reads_cannot_wedge_the_session() {
    let r = start(&[("FAKE_CLAUDE_MODE", "noread")], None).await;
    r.h.send("x".repeat(1_000_000)).unwrap();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let status = tokio::time::timeout(Duration::from_secs(5), r.h.status())
        .await
        .expect("写卡住时状态查询也必须能答")
        .unwrap();
    assert!(status.pid > 0);
    r.stop().await;
}

/// Review Focus 5：几十 KB 的开场任务（首轮白板）——只提交一次、一字不差，不因验证窗口
/// 超时重投出两份。
#[tokio::test]
async fn huge_brief_is_submitted_exactly_once() {
    let brief: String = (0..2000)
        .map(|i| format!("{i:05} 白板条目，带点中文\n"))
        .collect();
    assert!(brief.len() > 60_000);
    let mut r = start(&[], Some(&brief)).await;
    r.until(outcome(BRIEF_DELIVERY_ID, |o| {
        *o == DeliveryOutcome::Delivered
    }))
    .await;
    assert_eq!(r.submitted(), [brief.replace('\n', "\\n")]);
    r.stop().await;
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p crewd-exec --test fake_claude`
Expected: 编译失败，``environment variable `CARGO_BIN_EXE_fake_claude` not defined at compile time``。

- [ ] **Step 3: 写假 claude**

`crates/exec/src/bin/fake_claude.rs`（整个文件）：

```rust
//! 测试器材：一个只会画几屏的假 claude，跑在真 PTY 里（`tests/fake_claude.rs` 用）。
//! **不是产品代码**——只长到能让执行器的判据成立：输入框、粘贴折叠、信任框、限流菜单、额度报错。
//!
//! 环境变量（执行器只放行调用方显式给的变量，所以测试经 `ClaudeSpec::additional_env` 传）：
//! - `FAKE_CLAUDE_MODE`：`ready`（缺省）/ `deaf`（画框但永不回显）/ `noread`（画框后再也不读
//!   stdin，PTY 输入缓冲会被写满）/ `trust`（先过信任框）/ `ratelimit`（第一次提交弹限流菜单）/
//!   `usage`（第一次提交报额度用尽并退出）。
//! - `FAKE_CLAUDE_READY_DELAY_MS`：先只画横幅，这段时间收到的按键全部吞掉（真 claude 首屏重绘就这样吞字）。
//! - `FAKE_CLAUDE_REPLAY`：开场先原样回放这份录制字节（如 `tui-claude-trust.bin`）。
//! - `FAKE_CLAUDE_LOG`：每次提交往这个文件追加一行（正文里的换行写成 `\n`）。
//!
//! 粘贴判定照 claude（Swift 版在真 daemon 上实测到的行为）：同一次 read 里收到多个字节 = 粘贴，
//! 其中的 CR 当换行进输入框；粘贴之后单独来的回车只「结束粘贴」不提交，要先来一个普通按键
//! 才恢复按键语义——这就是执行器提交时要先发空格、退格再回车的原因。
use std::io::{Read, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const HINT: &str = "Try \"write a test\"";

struct Fake {
    mode: String,
    session: String,
    resumed: bool,
    input: String,
    last: String,
    log: Option<String>,
    submits: usize,
    pasting: bool,
}

fn out(bytes: &[u8]) {
    let mut o = std::io::stdout();
    let _ = o.write_all(bytes);
    let _ = o.flush();
}

impl Fake {
    fn banner(&self) -> String {
        format!(
            "\x1b[2J\x1b[HClaude Code (fake) session {} resumed={}\r\n",
            self.session, self.resumed
        )
    }

    fn draw_box(&self) {
        let shown = if self.input.is_empty() {
            HINT.to_string()
        } else if self.input.contains('\n') || self.input.chars().count() > 60 {
            format!("[Pasted text #1 +{} lines]", self.input.lines().count())
        } else {
            self.input.clone()
        };
        let rule = "─".repeat(40);
        out(format!(
            "{}\r\n{}\r\n\r\n{rule}\r\n\u{276f} {shown}\r\n{rule}\r\n  ? for shortcuts",
            self.banner(),
            self.last
        )
        .as_bytes());
    }

    fn draw_trust(&self, cursor: usize) {
        let mark = |i: usize| if i == cursor { "\u{276f}" } else { " " };
        out(format!(
            "\x1b[2J\x1b[HQuick safety check: Is this a project you created or one you trust?\r\n\r\n{}  No, exit\r\n{}  Yes, I trust this folder\r\n\r\nEnter to confirm \u{b7} Esc to cancel",
            mark(0),
            mark(1)
        )
        .as_bytes());
    }

    fn submit(&mut self) {
        let text = std::mem::take(&mut self.input);
        self.submits += 1;
        if let Some(path) = &self.log {
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .expect("打不开 FAKE_CLAUDE_LOG");
            let _ = writeln!(f, "{}", text.replace('\n', "\\n"));
        }
        let first: String = text.lines().next().unwrap_or("").chars().take(30).collect();
        self.last = format!("\u{23fa} GOT: {first}");
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let value_of = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let resumed = value_of("--resume");
    let mut fake = Fake {
        mode: std::env::var("FAKE_CLAUDE_MODE").unwrap_or_else(|_| "ready".into()),
        session: resumed
            .clone()
            .or_else(|| value_of("--session-id"))
            .unwrap_or_default(),
        resumed: resumed.is_some(),
        input: String::new(),
        last: String::new(),
        log: std::env::var("FAKE_CLAUDE_LOG").ok(),
        submits: 0,
        pasting: false,
    };

    let stdin = rustix::stdio::stdin();
    if let Ok(mut t) = rustix::termios::tcgetattr(stdin) {
        t.make_raw();
        let _ = rustix::termios::tcsetattr(stdin, rustix::termios::OptionalActions::Now, &t);
    }
    if fake.mode == "noread" {
        fake.draw_box();
        std::thread::sleep(Duration::from_secs(600));
        return;
    }
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        let mut input = std::io::stdin();
        while let Ok(n) = input.read(&mut buf) {
            if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                return;
            }
        }
    });

    if let Ok(path) = std::env::var("FAKE_CLAUDE_REPLAY") {
        let bytes = std::fs::read(&path).expect("读不到 FAKE_CLAUDE_REPLAY");
        for chunk in bytes.chunks(64) {
            out(chunk);
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    if fake.mode == "trust" {
        if std::env::var("FAKE_CLAUDE_REPLAY").is_err() {
            fake.draw_trust(0);
        }
        let mut cursor = 0;
        loop {
            let Ok(chunk) = rx.recv() else { return };
            if chunk.windows(3).any(|w| w == b"\x1b[B") {
                cursor = 1;
                fake.draw_trust(cursor);
            } else if chunk.windows(3).any(|w| w == b"\x1b[A") {
                cursor = 0;
                fake.draw_trust(cursor);
            } else if chunk.contains(&b'\r') {
                if cursor == 0 {
                    return;
                }
                break;
            }
        }
    }

    let delay: u64 = std::env::var("FAKE_CLAUDE_READY_DELAY_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if delay > 0 {
        out(fake.banner().as_bytes());
        let until = Instant::now() + Duration::from_millis(delay);
        while let Some(left) = until.checked_duration_since(Instant::now()) {
            let _ = rx.recv_timeout(left);
        }
    }
    fake.draw_box();
    if fake.mode == "deaf" {
        loop {
            if rx.recv().is_err() {
                return;
            }
        }
    }

    // 一个 UTF-8 字符可能被切在两次 read 之间：没收全的那几个字节留到下一块。
    let mut partial: Vec<u8> = Vec::new();
    while let Ok(chunk) = rx.recv() {
        let paste = chunk.len() > 1;
        if paste {
            fake.pasting = true;
        } else if chunk != b"\r" {
            fake.pasting = false;
        }
        partial.extend_from_slice(&chunk);
        let valid = match std::str::from_utf8(&partial) {
            Ok(_) => partial.len(),
            Err(e) if e.error_len().is_none() => e.valid_up_to(),
            Err(_) => partial.len(),
        };
        let rest = partial.split_off(valid);
        let text = String::from_utf8_lossy(&partial).into_owned();
        partial = rest;
        let mut submitted = false;
        for c in text.chars() {
            match c {
                '\r' if paste => fake.input.push('\n'),
                // 粘贴之后单独来的回车：只结束粘贴。
                '\r' if fake.pasting => {}
                '\r' => {
                    fake.submit();
                    submitted = true;
                }
                '\u{7f}' => {
                    fake.input.pop();
                }
                '\u{15}' => fake.input.clear(),
                c if c == '\n' || !c.is_control() => fake.input.push(c),
                _ => {}
            }
        }
        if submitted && fake.submits == 1 && fake.mode == "usage" {
            out(b"\r\nYou've reached your usage limit \xc2\xb7 resets 5pm\r\n");
            std::thread::sleep(Duration::from_millis(200));
            std::process::exit(1);
        }
        if submitted && fake.submits == 1 && fake.mode == "ratelimit" {
            out("\x1b[2J\x1b[HWhat do you want to do?\r\n\r\n\u{276f} 1. Stop and wait for limit to reset\r\n  2. Upgrade your plan\r\n\r\nEnter to confirm".as_bytes());
            let mut answer = String::new();
            while let Ok(chunk) = rx.recv() {
                answer.push_str(&String::from_utf8_lossy(&chunk));
                if answer.contains('\r') {
                    break;
                }
            }
            fake.input = format!("RATE-ANSWER:{}", answer.trim_end_matches('\r'));
            fake.submit();
        }
        fake.draw_box();
    }
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p crewd-exec --test fake_claude`（连跑三遍）
Expected: `10 passed`，每遍约 5s。

- [ ] **Step 5: 验三拍提交是被测到的**

`session.rs` 的 `Step::Submit` 分支里，把「写空格、排退格、排回车」那四行临时换成只写一个回车：

```rust
                self.actions.push(Action::Write(b"\r".to_vec()));
```

→ `cargo test -p crewd-exec --test fake_claude` → 红 7 条（约 22s）：`brief_waits_out_a_slow_first_screen_then_is_submitted_once`、`long_brief_folded_into_a_paste_placeholder_still_lands`、`steers_are_queued_and_each_submitted_once_in_order`、`recorded_trust_dialog_blocks_until_answered_then_brief_lands`、`rate_limit_menu_is_answered_automatically`、`usage_limit_then_exit_is_marked_hit_limit`、`huge_brief_is_submitted_exactly_once`。改回，再跑一次全绿。

- [ ] **Step 6: 写真机冒烟**

`crates/exec/tests/live_claude.rs`（整个文件）：

```rust
//! 真 claude 冒烟：只验「起得来、终端查询答得对（DA1 / OSC 11…）、画出输入框」，
//! **不发任何 prompt**（不花额度、不生成会话日志）。
//!
//! 两道闸，缺一不跑：`#[ignore]`（`check.sh` 不带 `--ignored`，永远不跑它）+
//! 环境变量 `CREWD_LIVE_CLAUDE=1`。工作目录取 `CREWD_LIVE_CLAUDE_WORKDIR`，缺省是本仓库根
//! （人平时就在这里用 claude，已信任，不会弹信任框；弹了也照样报出来，算失败）。
//!
//! 跑法：`CREWD_LIVE_CLAUDE=1 cargo test -p crewd-exec --test live_claude -- --ignored --nocapture`
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crewd_exec::driver::{ClaudeStart, DriverConfig, start_claude};
use crewd_exec::env::AgentPaths;
use crewd_exec::launch::ClaudeSpec;
use crewd_exec::resume::MemoryLedger;
use crewd_exec::session::SessionEvent;
use crewd_term::{blocking_dialog, input_row};

#[tokio::test]
#[ignore = "真 claude：CREWD_LIVE_CLAUDE=1 cargo test -p crewd-exec --test live_claude -- --ignored"]
async fn real_claude_starts_up_to_the_input_box() {
    if std::env::var("CREWD_LIVE_CLAUDE").as_deref() != Ok("1") {
        eprintln!("跳过：没设 CREWD_LIVE_CLAUDE=1");
        return;
    }
    let workdir = std::env::var_os("CREWD_LIVE_CLAUDE_WORKDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../..")));
    let paths = AgentPaths::discover().await;
    let program = paths.resolve("claude").expect("PATH 里没有 claude");
    let runtime = tempfile::tempdir().unwrap();
    let start = ClaudeStart {
        session_id: "live".into(),
        program,
        child_path: paths.child_path.clone(),
        spec: ClaudeSpec {
            workdir,
            ..ClaudeSpec::default()
        },
        ledger: Arc::new(MemoryLedger::default()),
    };
    let (h, mut rx) = start_claude(start, DriverConfig::new(runtime.path().to_path_buf()))
        .await
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut last = String::new();
    let ready = loop {
        if Instant::now() > deadline {
            break false;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        while let Ok(e) = rx.try_recv() {
            eprintln!("{e:?}");
            assert!(!matches!(e, SessionEvent::Exited(_)), "claude 起来就退了");
        }
        last = h.screen_text(50).await.unwrap();
        let rows: Vec<String> = last.lines().map(str::to_string).collect();
        assert!(
            blocking_dialog(&rows).is_none(),
            "弹了对话框（目录没信任？）：\n{last}"
        );
        if input_row(&rows).is_some() {
            break true;
        }
    };
    eprintln!("----- 最后一屏 -----\n{last}");
    h.stop().unwrap();
    while let Some(e) = rx.recv().await {
        if matches!(e, SessionEvent::Exited(_)) {
            break;
        }
    }
    assert!(
        ready,
        "60s 内没画出输入框（DA1 / OSC 回答不被接受时会卡在首屏）"
    );
}
```

- [ ] **Step 7: 确认它默认不跑**

Run: `cargo test -p crewd-exec --test live_claude`
Expected: `0 passed; 0 failed; 1 ignored`。再跑一次 `./scripts/check.sh` → 0，并在输出里确认 `live_claude` 那一行是 `1 ignored`。

- [ ] **Step 8: 真机跑一次（要人点头）**

这一步用本机已登录的 claude 起一个**不发 prompt** 的会话（工作目录缺省是本仓库根，会读用户自己的 claude 配置与全局 hooks）。先在群里问人能不能跑；人同意再执行：

```bash
CREWD_LIVE_CLAUDE=1 cargo test -p crewd-exec --test live_claude -- --ignored --nocapture
```

把结果（通过 / 失败、`claude --version`、最后一屏里输入框那一行、有没有弹信任框）记下来，Task 12 写进 spec §17。没跑就在 §17 写「未跑」，不许写成「已验证」。

- [ ] **Step 9: 提交**

```bash
git add crates/exec/src/bin/fake_claude.rs crates/exec/tests/fake_claude.rs crates/exec/tests/live_claude.rs
git commit -m "crewd-exec: fake claude test rig, end-to-end delivery tests and gated live smoke test

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 12: `crewd claude` 子命令、serve 回收孤儿、交付记录

把执行器接到 crewd 上：`crewd_server::reap_orphans`（读 `<数据目录>/exec-registry.json`、回收、返回登记表），`crewd serve` 拿到数据目录锁后先跑它；`crewd claude` 前台起一个会话——拿锁、回收孤儿、`AgentPaths::discover`、给了 `--session` 就用 `StoreLedger`（行必须存在；重跑就 `--resume`）、不给就 `MemoryLedger`，事件逐行打到 stdout，stdin 每一行当一条指令投进去，Ctrl-C / SIGTERM 停掉会话，`--screen` 时每次转入空闲把屏幕打到 stderr。这就是 spec §3.3「每个 M 都以集成测试 + CLI 可驱动为完成标准」的 M2b 那一半。

**Files:**
- Create: `crewd/crates/crewd/src/claude_cmd.rs`、`crewd/crates/crewd/tests/claude_cli.rs`
- Modify: `crewd/crates/server/src/lib.rs`、`crewd/crates/crewd/src/main.rs`、`crewd/crates/crewd/Cargo.toml`、`crewd/Cargo.lock`、`crewd/README.md`、`docs/superpowers/specs/2026-10-05-crewd-backend-and-work-tracking-design.md`

**Interfaces:**
- Consumes: Task 7 `registry::{ProcessRegistry, OrphanReport, reap_orphans}`；Task 9 `StoreLedger`、`MemoryLedger`；Task 10 窄接口；Task 1 `AgentPaths`；Task 5 `AuthProbe::projects_dir_for`。
- Produces:
  - `crewd_server::PROCESS_REGISTRY_FILE: &str`（`"exec-registry.json"`）、`crewd_server::reap_orphans(data_dir: &Path) -> anyhow::Result<(Arc<ProcessRegistry>, Vec<OrphanReport>)>`（调用前必须持有数据目录锁）
  - 子命令 `crewd claude --workdir DIR [--session SID] [--prompt TEXT] [--model M] [--effort E] [--claude PATH] [--screen]`

- [ ] **Step 1: 写失败的测试**

`crates/crewd/tests/claude_cli.rs`（整个文件；「claude」是打印参数后退出的 shell 脚本，`SHELL=/usr/bin/false` 让登录 shell 抓 PATH 走失败分支）：

```rust
//! `crewd claude` 的胶水：库里的会话号账本 → 第一次新号、第二次 `--resume` 同一个号。
//! 「claude」用 shell 脚本顶替（打印参数后退出）。
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use crewd_proto::{Crew, Session};
use crewd_store::{Store, crews, sessions};

fn seed(data: &Path) {
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
            captain_agent_kind: None,
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
            executor: "claude".into(),
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

fn run(data: &Path, fake: &Path) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_crewd"))
        .env("CREWD_DATA_DIR", data)
        // 登录 shell 抓 PATH 走失败分支：测试不依赖本机的 rc 文件。
        .env("SHELL", "/usr/bin/false")
        .args(["claude", "--session", "w-1", "--workdir"])
        .arg(data)
        .arg("--claude")
        .arg(fake)
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn second_run_resumes_the_id_recorded_by_the_first() {
    let data = tempfile::tempdir().unwrap();
    seed(data.path());
    let fake = data.path().join("fake-claude");
    std::fs::write(&fake, "#!/bin/sh\necho \"ARGS $*\"\nsleep 0.2\n").unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();

    let first = run(data.path(), &fake);
    assert!(first.contains("resumed: false"), "{first}");
    assert!(first.contains("Exited("), "{first}");
    let recorded = {
        let s = Store::open(&data.path().join("crewd.db")).unwrap();
        sessions::agent_session_id(s.conn(), "w-1")
            .unwrap()
            .expect("第一次起之前就该落账")
    };
    assert!(first.contains(&recorded), "{first}");

    let second = run(data.path(), &fake);
    assert!(
        second.contains(&format!("agent_session_id: \"{recorded}\", resumed: true")),
        "{second}"
    );
    let registry = std::fs::read_to_string(data.path().join("exec-registry.json")).unwrap();
    assert_eq!(registry.trim(), "{}", "退出后登记表清空");
}
```

`crates/server/src/lib.rs` 测试模块里、`second_lock_on_same_dir_is_refused` 之前加（**不要在这里起子进程**：fork 到 exec 之间子进程攥着同一个锁文件描述符，会让并行跑的加锁测试偶发失败——写计划时第一版就这样红过一次）：

```rust
    #[test]
    fn startup_reaps_registry_entries_and_clears_them() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(PROCESS_REGISTRY_FILE);
        let reg = ProcessRegistry::load(&path).unwrap();
        // 不存在的 pid（超出 macOS / Linux 的 pid 上限）→ 只清账。别在这里起子进程：
        // fork 到 exec 之间子进程攥着同一个锁文件描述符，会让并行跑的加锁测试偶发失败。
        let pid = i32::MAX;
        reg.record(
            "gone",
            crewd_exec::registry::ProcessIdentity {
                pid,
                pgid: pid,
                start_time: 1,
                command: "claude".into(),
            },
        )
        .unwrap();
        drop(reg);
        let (registry, reports) = reap_orphans(root.path()).unwrap();
        assert_eq!(reports.len(), 1);
        assert_eq!(
            reports[0].decision,
            crewd_exec::registry::OrphanDecision::AlreadyGone
        );
        assert!(registry.entries().is_empty());
        assert!(
            ProcessRegistry::load(&path).unwrap().entries().is_empty(),
            "清账落盘"
        );
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p crewd-server startup_reaps`
Expected: 编译失败，``cannot find value `PROCESS_REGISTRY_FILE` in this scope``。

Run: `cargo test -p crewd --test claude_cli`
Expected: 运行时失败，断言里带 clap 的 ``unrecognized subcommand 'claude'``。

- [ ] **Step 3: serve 侧**

`crates/server/src/lib.rs`：在 `use crewd_store::Store;` 之前加

```rust
use crewd_exec::registry::{OrphanReport, ProcessRegistry};
```

在 `pub async fn serve(` 之前加：

```rust
/// 孤儿登记表的位置（执行器起的每个 agent 进程都登记在这里，退出时清账）。
pub const PROCESS_REGISTRY_FILE: &str = "exec-registry.json";

/// 读登记表并回收上次崩溃留下的孤儿。**必须先拿到数据目录锁**（确认没有别的 crewd 活着）。
/// 返回登记表（之后起的会话继续往里登记）和每一条的处置说明。
pub fn reap_orphans(data_dir: &Path) -> anyhow::Result<(Arc<ProcessRegistry>, Vec<OrphanReport>)> {
    let registry = Arc::new(ProcessRegistry::load(
        &data_dir.join(PROCESS_REGISTRY_FILE),
    )?);
    let reports = crewd_exec::registry::reap_orphans(&registry)?;
    Ok((registry, reports))
}
```

`serve` 里 `let lock = lock_data_dir(&data_dir)?;` 之后加：

```rust
    // M3 起会话由 serve 拉起，登记表交给会话管理；M2b 先只在启动时回收。
    let (_registry, reports) = reap_orphans(&data_dir)?;
    for r in reports {
        eprintln!("crewd: {}", r.message);
    }
```

Run: `cargo test -p crewd-server`
Expected: 单测 `5 passed`，`socket` 集成测试照旧全绿。

- [ ] **Step 4: `crewd claude`**

```bash
cargo add --offline -p crewd crewd-exec --path crates/exec
cargo add --offline -p crewd tokio --features io-std,io-util
```

`crates/crewd/src/claude_cmd.rs`（整个文件）：

```rust
//! `crewd claude`：前台起一个 claude 会话（M2b 的验收与调试入口；M3 起由 serve 拉起会话）。
//! 事件逐行打到 stdout；stdin 每一行当一条指令投进去；Ctrl-C / SIGTERM 停掉会话。
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use crewd_exec::driver::{ClaudeStart, DriverConfig, start_claude};
use crewd_exec::env::AgentPaths;
use crewd_exec::health::AuthProbe;
use crewd_exec::launch::ClaudeSpec;
use crewd_exec::resume::{AgentIdLedger, MemoryLedger};
use crewd_exec::session::SessionEvent;
use crewd_server::ledger::StoreLedger;
use tokio::io::AsyncBufReadExt;

#[derive(clap::Args, Clone)]
pub struct ClaudeArgs {
    /// 工作目录
    #[arg(long)]
    workdir: PathBuf,
    /// crewd session id：给了就用库里的会话号账本（行必须已存在，重跑会 --resume）；不给只在内存里记
    #[arg(long)]
    session: Option<String>,
    /// 开场任务（走 PTY 投递，绝不进 argv）
    #[arg(long)]
    prompt: Option<String>,
    #[arg(long)]
    model: Option<String>,
    #[arg(long)]
    effort: Option<String>,
    /// claude 可执行文件；缺省按登录 shell 的 PATH 找
    #[arg(long)]
    claude: Option<PathBuf>,
    /// 每次转入空闲时把当前屏幕打到 stderr
    #[arg(long)]
    screen: bool,
}

pub async fn run(data_dir: &Path, args: ClaudeArgs) -> anyhow::Result<()> {
    crewd_server::ensure_data_dir(data_dir)?;
    // 与 serve / import 同一把锁：账本写库、回收孤儿都要求没有别的 crewd 活着。
    let _lock = crewd_server::lock_data_dir(data_dir)?;
    let (registry, reports) = crewd_server::reap_orphans(data_dir)?;
    for r in reports {
        eprintln!("crewd: {}", r.message);
    }
    // 子进程的 cwd 是 workdir：相对路径在那边会被按 workdir 解析，先在这里转成绝对路径。
    let workdir = std::fs::canonicalize(&args.workdir)
        .with_context(|| format!("工作目录 {} 不可用", args.workdir.display()))?;
    let paths = AgentPaths::discover().await;
    let program = match &args.claude {
        Some(p) => std::fs::canonicalize(p).with_context(|| format!("找不到 {}", p.display()))?,
        None => paths
            .resolve("claude")
            .context("登录 shell 的 PATH 里找不到 claude")?,
    };
    let ledger: Arc<dyn AgentIdLedger> = match &args.session {
        Some(s) => Arc::new(StoreLedger::new(data_dir.join("crewd.db"), s.clone())),
        None => Arc::new(MemoryLedger::default()),
    };
    let mut config = DriverConfig::new(data_dir.join("run"));
    config.registry = Some(registry);
    config.claude_projects_dir = AuthProbe::projects_dir_for(
        std::env::var("CLAUDE_CONFIG_DIR").ok().as_deref(),
        std::env::var("HOME").ok().as_deref(),
        &workdir,
    );
    let start = ClaudeStart {
        session_id: args.session.unwrap_or_else(|| "cli".to_string()),
        program,
        child_path: paths.child_path,
        spec: ClaudeSpec {
            workdir,
            model: args.model,
            effort: args.effort,
            initial_prompt: args.prompt,
            ..ClaudeSpec::default()
        },
        ledger,
    };
    let (handle, mut events) = start_claude(start, config).await?;
    let mut lines = tokio::io::BufReader::new(tokio::io::stdin()).lines();
    let mut stdin_open = true;
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    loop {
        tokio::select! {
            ev = events.recv() => {
                let Some(ev) = ev else { break };
                println!("{ev:?}");
                if args.screen
                    && ev == SessionEvent::Working(false)
                    && let Ok(text) = handle.screen_text(50).await
                {
                    eprintln!("----- screen -----\n{text}\n------------------");
                }
                if matches!(ev, SessionEvent::Exited(_)) {
                    break;
                }
            }
            line = lines.next_line(), if stdin_open => match line {
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

`crates/crewd/src/main.rs` 三处：文件头注释之后加 `mod claude_cmd;`（后跟一个空行）；`enum Cmd` 末尾（`Schema,` 之后）加

```rust
    /// 前台起一个 claude 会话（M2b 验收 / 调试；会话事件打到 stdout）
    Claude(claude_cmd::ClaudeArgs),
```

`run()` 的 `match &cli.cmd` 里、`Cmd::Schema => {` 之前加

```rust
        Cmd::Claude(args) => claude_cmd::run(&dir, args.clone()).await?,
```

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -p crewd --test claude_cli`
Expected: `1 passed`（约 1s）。

Run: `cargo run -p crewd -- claude --help`
Expected: 列出 `--workdir`（必填）、`--session`、`--prompt`、`--model`、`--effort`、`--claude`、`--screen`。

- [ ] **Step 6: 全量检查并提交代码**

Run: `./scripts/check.sh` → 0（全仓：crewd-exec 单测 81、`driver` 11、`fake_claude` 10 + 1 ignored、crewd-server 单测 5、crewd-store 单测 11、`claude_cli` 1）。

```bash
git add Cargo.lock crates/server/src/lib.rs crates/crewd/Cargo.toml crates/crewd/src/main.rs crates/crewd/src/claude_cmd.rs crates/crewd/tests/claude_cli.rs
git commit -m "crewd: claude subcommand drives one session from the CLI; serve reaps orphans at startup

Co-Authored-By: Claude <noreply@anthropic.com>"
```

- [ ] **Step 7: 手动从命令行驱动一次（不用真 claude）**

用假 claude 走一遍人会走的路：

```bash
cargo build -p crewd-exec --bin fake_claude
D=$(mktemp -d)
printf '第二条指令\n' | cargo run -q -p crewd -- --data-dir "$D/data" claude --workdir "$D" --claude target/debug/fake_claude --prompt "第一条" --screen
```

Expected: stdout 依次出现 `Spawned { … resumed: false }`、`Delivery { id: 0, … outcome: Delivered }`、`Delivery { id: 1, … outcome: Delivered }`；stderr 的屏幕块里能看到 `⏺ GOT: 第二条指令`。stdin 读完后会话不退出——按 Ctrl-C，应看到 `Exited(ExitInfo { … stopped_by_user: true … })` 后进程结束。（不带 `--session` 时账本只在内存里，不碰库。）

- [ ] **Step 8: Linux 编译检查（可选，要人点头装 target）**

本机没有 Linux target 时要先 `rustup target add x86_64-unknown-linux-gnu`（会下载约几十 MB，改的是本机工具链——先问人）。装了就跑：

```bash
cargo clippy -p crewd-exec --all-targets --target x86_64-unknown-linux-gnu -- -D warnings
```

只编译、不链接，不需要交叉 C 工具链（crewd-exec 的依赖里没有 C 代码）。它覆盖的是 `#[cfg(not(target_os = "macos"))]` 的 `/proc` 读法和 Linux 才编译的 driver 单测；结果写进 §17。

- [ ] **Step 9: 交付记录**

`crewd/README.md`：在「终端核心」那一行之后加两行：

```markdown
- 执行器：`crates/exec`（起 claude 会话：PTY、开场投递、健康、停止与孤儿回收、`--resume`；对外只有 `driver::SessionHandle` + 事件流）
- 前台起一个 claude 会话：`cargo run -p crewd -- claude --workdir DIR [--session SID] [--prompt TEXT] [--screen]`（给了 `--session` 会话号就记在库里那一行上，重跑自动 `--resume`；会拿数据目录锁，不能与 serve 同时跑）
```

spec `docs/superpowers/specs/2026-10-05-crewd-backend-and-work-tracking-design.md` §3.3 里 M2 那一条的开头

```markdown
- **M2 执行器**：portable-pty + alacritty_terminal 无头终端（屏幕文本、快照、对话框识别）
```

换成

```markdown
- **M2 执行器**：PTY（rustix-openpty 手写 + tokio，选型见 §17）+ alacritty_terminal 无头终端（屏幕文本、快照、对话框识别）
```

（该行其余部分不动。）然后在文件末尾追加下面这一节，并按 Task 11 Step 8、本任务 Step 8 的实际结果把两处「二选一」改成一句真话（版本号、秒数照实填；没跑就留「未跑」那句）：

```markdown
## 17. M2b 交付记录与遗留（分支 crewd-m2b，计划 `docs/superpowers/plans/2026-10-06-crewd-m2b-claude-session.md`）

已交付：`crewd/crates/exec`（crewd-exec）——找 claude（登录 shell PATH + 哨兵 + 5s 超时、childProcessPath、白名单环境）、argv 与 0600 启动文件（首条 prompt 绝不进 argv）、hooks / MCP 命令行指向可配置 helper（`HelperCommands` 一处拼；hooks 可关，等 M3 的 `crewd hook`）、手写 PTY（rustix-openpty + tokio `AsyncFd`，setsid / TIOCSCTTY / env_clear）、sans-IO 会话核心 `session::Core`（投递状态机、健康、待决策、退出，时间注入）、驱动任务与窄接口 `driver::SessionHandle` + `SessionEvent`、孤儿登记与启动时刻核对的回收（macOS / Linux）、会话号账本 trait + `crewd_server::ledger::StoreLedger`、续跑被拒换新号重起一次。`crewd claude` 子命令（前台起一个会话，事件打到 stdout）；`crewd serve` 启动时回收孤儿。schema v2：`sessions.agent_kind` → `executor`（`claude` / `codex` / `acp:<registry-id>`，旧数据的 `terminal` / `unknown` 原样）。验收：假 claude（真 PTY）上的端到端——慢首屏、粘贴折叠、排队、粘贴陷阱、永不回显、真录制信任框回放后代答补投、限流菜单自动应答、额度用尽退出、子进程不读输入、70KB 开场任务；shell 脚本顶替 claude 的驱动层测试——落账、续跑、被拒重起、停止升级 killpg、拉起失败、登记表、认证探测、账本失败不起进程；`crewd claude` 两次运行之间经库续跑。

与 Swift / 本 spec 原文不同的决定（理由见计划「裁定」）：
- PTY 手写，不用 portable-pty（§3.3 原写法，已改）也不用 `alacritty_terminal::tty`（继承父环境且加 `ALACRITTY_WINDOW_ID`、cwd 失败静默、Drop 里阻塞 wait）。
- §15 的 CRLF 问题：字节尾窗不改；执行器判待决策只看屏幕（不喂跟踪器的尾窗）。折掉 CRLF 会让答完的菜单在尾窗里挂着，把「已清除」拖住。额度 / 限流 / 续跑被拒这些单行短语仍用尾窗。
- 输入行连续在屏 300ms 才算就绪：`tui-claude-trust.bin` 按 64 字节喂，第 1024 字节处信任框半帧里 `❯  No, exit` 被认成输入行。
- 被对话框挡住 / 等不到输入框只报不终止，答完自己补投（Swift 驱动层把这两种当终局、丢了开场任务——与它自己状态机的测试矛盾）。
- 提交三拍发完才回读（粘贴占位符会把那个空格算成多一行）；正文里的 CR 变 LF、其余控制字符变 Unicode 控制图形；TERM 固定 `xterm-256color`；额外环境变量多拒一个 `CREWD_` 前缀。
- 续跑被拒同时看屏幕和字节尾窗（窄屏时那行原话会折行）。
- 孤儿启动时刻在 macOS 上取 `proc_pidinfo(PROC_PIDTBSDINFO)`（与 sysctl `KERN_PROC` 同一个内核字段；libc 没给 macOS 导出 `kinfo_proc`）。

真机核对（§15 遗留的 DA1 一条）：
- 【执行 Task 11 Step 8 后二选一，删掉另一条】已跑：claude <版本>，`CREWD_LIVE_CLAUDE=1 cargo test -p crewd-exec --test live_claude -- --ignored` 通过——alacritty 的 DA1 / OSC 11 回答被接受，<秒数>s 内画出输入框，没弹对话框。
- 【或】未跑：DA1 / 主题选择仍只在录制字节上验过；跑法见 `crewd/crates/exec/tests/live_claude.rs` 文件头。

遗留：
- `crews.captain_agent_kind` 仍是旧词表（`claude_code`）；§16 只说了 session。M3 定机长执行器时一起改。
- 提交三拍与正文验证窗口都从「排进写任务」起算，不是「真正写进 PTY」：子进程长时间不读输入时，空格 / 退格 / 回车可能挤进同一次 read 被当成粘贴，超长正文也可能在窗口内没写完而被 Ctrl-U 重投。假 claude 上 70KB 开场任务没出过事；真遇到了就让写任务回报「写完了」再排下一拍。
- 只在启动时回收孤儿；登记表交给会话管理、serve 拉起会话是 M3。`crewd claude` 拿数据目录锁，不能与 serve 同时跑。
- server 的 RPC 仍在 tokio 线程上同步调 SQLite（§14 遗留）；M2b 的账本另开连接、在 `spawn_blocking` 里跑，没加重它。M3 把会话放进 serve 之前处理。
- 续跑被拒只做了判定；Swift 那套诊断文案（日志在不在盘上）与「新开一轮」的群通知归 M3。
- `/model` 中途切换（`SessionProfileSwitch`）、启动参数回显扫描、awaitingReply 归 M3。
- `TIOCSCTTY` 与「退出后读干净」只在 Linux 上有可观测差别，macOS 上的测试证明不了它们（计划 Task 3 / Task 10 有说明）。
- Linux：【执行 Task 12 Step 8 后二选一，删掉另一条】`cargo clippy -p crewd-exec --all-targets --target x86_64-unknown-linux-gnu -- -D warnings` 通过（只编译，没在 Linux 上跑测试）。/ 没在 Linux 上编译过。
```

- [ ] **Step 10: 提交文档**

```bash
git add README.md ../docs/superpowers/specs/2026-10-05-crewd-backend-and-work-tracking-design.md
git commit -m "docs: M2b delivery record (spec section 17), PTY choice in section 3.3, README

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

## 覆盖对照（M2b 范围 → 任务）

| 范围项（来源） | 任务 |
|---|---|
| 登录 shell PATH（哨兵、5s 超时）、childProcessPath、找可执行文件（移植图 §1） | 1 |
| 环境白名单、禁用前缀、TERM（移植图 §1、§8） | 1 |
| argv（`--resume` / `--session-id`、auto、system-prompt-file、settings、mcp-config、model、effort）、prompt 不进 argv、fast 模式（移植图 §1） | 2 |
| 0600 临时文件、helper 路径参数化、hooks 可关、M3 接口（`HelperCommands`）（移植图 §1、§8） | 2 |
| PTY：setsid、TIOCSCTTY、SIGPIPE 复位、进程组、非阻塞主端、tokio 读写、选型（移植图 §1、§8、§9.5） | 3 |
| 投递状态机：就绪 / 落地 / 三拍提交 / Ctrl-U / 3 次 / 对话框 3s / 90s / 0.5–2.5s（移植图 §2） | 4、6 |
| 一次一笔、排队；退出丢弃留账；已退出时投递报账（移植图 §2） | 6 |
| 额度短语、限流菜单自动应答 + 30s 冷却、额度恢复 6s（移植图 §2、§6） | 5、6 |
| 认证探测（`projects/*/<id>.jsonl`，2s，目录可配置）（移植图 §6） | 5、10 |
| 拉起自检 25s 与四种裁决（移植图 §5、§6） | 5、6、10 |
| 干活 / 空闲边沿、待决策出现 / 清除、状态推导（移植图 §2、§6） | 6 |
| 节拍：每块输出 + 0.6s + 精确唤醒；时间注入（移植图 §2；spec §13 Q12） | 6、10 |
| 窄接口：启动 / 输入 / 停止 / 状态订阅 / 屏幕文本 / 快照（spec §13 Q12） | 10 |
| 停止：SIGTERM → 2s → killpg SIGKILL；退出状态解码；hit_limit（移植图 §5、§6） | 6、10 |
| 孤儿登记与回收，启动时刻核对，macOS / Linux（移植图 §5、§8） | 7、12 |
| 会话号账本在 M1 store、重启 `--resume`（spec §13 Q12、移植图 §5） | 9、10、12 |
| 续跑被拒 5s 内识别 + 换新号重起一次（移植图 §2、§5） | 6、10 |
| 执行器类型字段 `claude` / `codex` / `acp:<id>`（spec §16） | 8 |
| CRLF 字节尾窗的决定（spec §15） | 6（裁定 3）、12（§17） |
| DA1 回答真机核对（spec §15） | 11（Step 8）、12（§17） |
| 假 claude 端到端、真机冒烟 opt-in 且不进 check.sh | 11 |
| CLI 可驱动（spec §3.3） | 12 |
