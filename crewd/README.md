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
- **从不在 CI 跑 `--ignored`**。被跳过的 6 个（nextest 报「6 tests skipped」）：4 个是要真东西的活测试——`crewd-exec` 的 `live_claude::real_claude_starts_up_to_the_input_box`、`live_codex::live_codex_handshakes_and_opens_a_thread_without_a_turn`、`live_codex::live_codex_turn_sees_the_untrusted_whiteboard_context`（要真 claude / codex 与账号），`crewd-jev` 的 `key::tests::keychain_roundtrip`（要真钥匙串，且仅 macOS）；另 2 个是 `crewd-store` 的量级基准 `fsync_cost::commit_cost_with_and_without_fullfsync`、`check_cost::check_and_snapshot_cost_at_real_scale`（手动跑，读数记进 N4 报告）。本机手动跑法见各测试的 `#[ignore]` 原因。
- **Linux 不排除任何可移植测试**：钥匙串（`security-framework`）与 `registry.rs` 里 macOS 的进程查询都有 `cfg(not(target_os = "macos"))` 的对应实现，Linux 上整套非 ignore 测试在容器里实测全过过一次（N0 当时 952 个；此后没在 Linux 重跑。N1 合 main 前（N1b-2）macOS 上 `scripts/check.sh` 跑 1111 个：1108 个测试 + 3 个 doctest，另 6 个 ignored；`registry::tests::reaper_kills_a_live_orphan_group_but_not_a_reused_pid` 在 Linux 上被 nextest 标 LEAK——测试故意留孤儿进程组，不影响通过）。Linux 这份的价值是抓「无意间依赖 macOS」的假设；产品的权威平台仍是 macOS 那份。
