# crewd

PendingCrew 的跨平台后端（Rust）。设计：`../docs/superpowers/specs/2026-10-05-crewd-backend-and-work-tracking-design.md`。

- 检查：`scripts/check.sh`
- 终端核心：`crates/term`（alacritty_terminal 无头终端：读屏、claude 对话框/输入框识别、待决策跟踪、快照；纯计算，不起进程、不用 tokio）
- Jev 客户端：`crates/jev`（派活 / 叫醒 / 归属 / 紧急度四个判断；发前脱敏；本机诊断日志不记 key）。给 Jev 看的文字全在 `crates/jev/prompts/*.md`
- Jev 评测（真 API、花钱、不进 check.sh）：`cargo run -p crewd-jev --bin jev-eval -- --seeds 2 --out crates/jev/eval/results/<日期>.json`；key 取 `JEV_API_KEY`，否则钥匙串 service `pendingcrew-jev` account `default`。改了 `prompts/` 的措辞或 `eval/cases.zh.json` 后重跑，读数记进 spec 的「M5a 交付记录与中文实测」一节
- 导入旧数据（只读源目录，只写新库）：`cargo run -p crewd -- import [--from DIR]`
- 运行：`cargo run -p crewd -- serve`
- 调用：`cargo run -p crewd -- rpc crew.list`
- API 契约：`schema/crewd-api.schema.json`（改了线协议类型后用 `cargo run -p crewd -- schema > schema/crewd-api.schema.json` 重新生成；测试会检查它是否过期）
- 数据目录：`--data-dir` > `CREWD_DATA_DIR` > `~/Library/Application Support/crewd`
