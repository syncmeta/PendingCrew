# crewd

PendingCrew 的跨平台后端（Rust）。设计：`../docs/superpowers/specs/2026-10-05-crewd-backend-and-work-tracking-design.md`。

- 检查：`scripts/check.sh`
- 终端核心：`crates/term`（alacritty_terminal 无头终端：读屏、claude 对话框/输入框识别、待决策跟踪、快照；纯计算，不起进程、不用 tokio）
- 执行器：`crates/exec`（起 claude 会话：PTY、开场投递、健康、停止与孤儿回收、`--resume`；对外只有 `driver::SessionHandle` + 事件流）
- 前台起一个 claude 会话：`cargo run -p crewd -- claude --workdir DIR [--session SID] [--prompt-file PATH|-] [--screen]`（开场任务从文件或 stdin（`-`）读，绝不放命令行参数——`ps` 看得见；给了 `--session` 会话号就记在库里那一行上，重跑自动 `--resume`；会拿数据目录锁，不能与 serve 同时跑）
- 工作跟进核心：`crates/work`（两层 Todo、工作项 DAG 与不变量、负责人、检查点、停下即检查、等人升级、巡检判定；纯计算，不起进程、不用 tokio、不读时钟；持久化与 MCP 接线在 M4b）
- 导入旧数据（只读源目录，只写新库）：`cargo run -p crewd -- import [--from DIR]`
- 运行：`cargo run -p crewd -- serve`
- 调用：`cargo run -p crewd -- rpc crew.list`
- API 契约：`schema/crewd-api.schema.json`（改了线协议类型后用 `cargo run -p crewd -- schema > schema/crewd-api.schema.json` 重新生成；测试会检查它是否过期）
- 数据目录：`--data-dir` > `CREWD_DATA_DIR` > `~/Library/Application Support/crewd`
