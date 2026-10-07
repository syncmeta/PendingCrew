# crewd

PendingCrew 的跨平台后端（Rust）。设计：`../docs/superpowers/specs/2026-10-05-crewd-backend-and-work-tracking-design.md`。

- 检查：`scripts/check.sh`（本机缺省 `cargo test`；`CREWD_TEST_RUNNER=nextest` 走 nextest `ci` 档，CI 就是这样调的，见下「CI」）
- 终端核心：`crates/term`（alacritty_terminal 无头终端：读屏、claude 对话框/输入框识别、待决策跟踪、快照；纯计算，不起进程、不用 tokio）
- 执行器：`crates/exec`（起 claude 会话：PTY、开场投递、健康、停止与孤儿回收、`--resume`；对外只有 `driver::SessionHandle` + 事件流）
- 前台起一个 claude 会话：`cargo run -p crewd -- claude --workdir DIR [--session SID] [--prompt-file PATH|-] [--screen]`（开场任务从文件或 stdin（`-`）读，绝不放命令行参数——`ps` 看得见；给了 `--session` 会话号就记在库里那一行上，重跑自动 `--resume`；会拿数据目录锁，不能与 serve 同时跑）
- codex 执行器：`crates/exec/src/codex`（`codex app-server` 客户端：握手、thread 落账与续跑、回合、服务端请求全答、协议健康、进程组；对外仍是 `driver::SessionHandle` + 事件流）
- 前台起一个 codex 会话：`cargo run -p crewd -- codex --workdir DIR [--session SID] [--prompt-file PATH|-] [--transcript]`（开场任务从文件或 stdin（`-`）读，绝不放命令行参数；给了 `--session` 则 thread id 记在库里那一行上，重跑自动 `thread/resume`；那一行的执行器必须是 `codex`；会拿数据目录锁，不能与 serve 同时跑）
- 工作跟进核心：`crates/work`（两层 Todo、工作项 DAG 与不变量、负责人、检查点、停下即检查、等人升级、巡检判定；纯计算，不起进程、不用 tokio、不读时钟；持久化与 MCP 接线在 M4b）
- crates/collab（crewd-collab）：协作层——群、成员、群主、消息与 @ 解析、游标与红标、机组 DAG、人类待确认（H 号经 crewd-work）、审批、重启询问、唤醒队列与熔断台账、渲染（M3a）。
- Jev 客户端：`crates/jev`（派活 / 叫醒 / 归属 / 紧急度四个判断；发前脱敏；本机诊断日志不记 key）。给 Jev 看的文字全在 `crates/jev/prompts/*.md`
- Jev 评测（真 API、花钱、不进 check.sh）：`cargo run -p crewd-jev --bin jev-eval -- --seeds 2 --out crates/jev/eval/results/<日期>.json`；key 取 `JEV_API_KEY`，否则钥匙串 service `pendingcrew-jev` account `default`。改了 `prompts/` 的措辞或 `eval/cases.zh.json` 后重跑，读数记进 spec 的「M5a 交付记录与中文实测」一节
- 导入旧数据（只读源目录，只写新库）：`cargo run -p crewd -- import [--from DIR]`
- 运行：`cargo run -p crewd -- serve`
- 调用：`cargo run -p crewd -- rpc crew.list`
- API 契约：`schema/crewd-api.schema.json`（改了线协议类型后用 `cargo run -p crewd -- schema > schema/crewd-api.schema.json` 重新生成；测试会检查它是否过期）
- 数据目录：`--data-dir` > `CREWD_DATA_DIR` > `~/Library/Application Support/crewd`

## CI

`.github/workflows/crewd.yml`（与 Xcode 那条 `ci.yml` 分开）：push main 必跑，PR 在动了 `crewd/**` 或该 workflow 时跑，也可手动触发。macOS（`macos-26`）与 Linux（`ubuntu-latest`）各一份，**都跑同一个** `scripts/check.sh`（`CREWD_TEST_RUNNER=nextest`）：`cargo fmt --check`、`clippy --all-targets -D warnings`、`cargo nextest run --profile ci`、`cargo test --doc`。工具链只认 `rust-toolchain.toml`，nextest 版本钉在 workflow 的 `NEXTEST_VERSION`。

- **抖动即红**：`.config/nextest.toml` 的 `ci` 档 `retries = 2` + `flaky-result = "fail"`——重试只为把抖动标出来（报告里 `FLAKY`、JUnit 里 `<flakyFailure>`、job 摘要里列名），不是吞掉。JUnit 作为 artifact 上传。
- **从不在 CI 跑 `--ignored`**。被跳过的 4 个（nextest 报「4 tests skipped」）全是要真东西的活测试：`crewd-exec` 的 `live_claude::real_claude_starts_up_to_the_input_box`、`live_codex::live_codex_handshakes_and_opens_a_thread_without_a_turn`、`live_codex::live_codex_turn_sees_the_untrusted_whiteboard_context`（要真 claude / codex 与账号），`crewd-jev` 的 `key::tests::keychain_roundtrip`（要真钥匙串，且仅 macOS）。本机手动跑法见各测试的 `#[ignore]` 原因。
- **Linux 不排除任何可移植测试**：钥匙串（`security-framework`）与 `registry.rs` 里 macOS 的进程查询都有 `cfg(not(target_os = "macos"))` 的对应实现，Linux 上整套 952 个非 ignore 测试在容器里实测全过（`registry::tests::reaper_kills_a_live_orphan_group_but_not_a_reused_pid` 在 Linux 上被 nextest 标 LEAK——测试故意留孤儿进程组，不影响通过）。Linux 这份的价值是抓「无意间依赖 macOS」的假设；产品的权威平台仍是 macOS 那份。
