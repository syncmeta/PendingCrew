# Codex 首版本机交付清单

先让个人用户能在独立新版里与 Codex 共事，再用新版推进完整工作模型。完整自由表、行政巡检、Claude ACP、其他端不是本机首版前置；Jev/记忆仍先讨论。此清单用于冻结和交接，不替代运行验收。

## 冻结产物

选择且只指定一个交付应用目录及其 `source-head.txt`。保留同目录空 `source-changes.patch`、`artifact-sha256.txt` 与原始验收日志。执行：

```sh
sh scripts/dev/check-codex-first-artifact.sh '/absolute/path/PendingCrew Codex First.app'
```

脚本只读检查 plist、GUI/helper 可执行文件、两层签名、源码记录和实际 hash；不会启动后台、模型或读取登录凭据。搬迁产物时应用和三个 metadata 文件须一起保留。通过只证明产物与记录一致，不证明 Git SHA 构建来源、正式签名/公证、安装或真实运行；冻结构建仍须保留对应构建日志。

## 一套输入的使用闭环

冻结 GUI/helper → 私有 HOME 和数据根的合成客户端 → 同一 helper 的正常退出/强制退出恢复 → 原生窗口操作。覆盖：建机组/选目录、发任务与群消息、原生权限和提问、旧答复拒绝、停止/打断/恢复、重连不重放、失败可见、人工决定后台恢复、停止成员不复活。使用现有 `test-codex-first-client.sh`、`test-codex-first-restart.sh`、`test-codex-first-shutdown.sh` 的断言；这些脚本默认构建 debug helper，不能自动称为已测打包 release helper。没有源码变化不重复无关全量门禁。

每项标明 source / packaged / synthetic / real / installed。9424c207 的恢复窗口与后台组合证据见既有[候选记录](2026-10-12-codex-first-local-candidate.md)。4d111668 的 release helper/client 合成证据为 `/private/tmp/pcw-work-idle-release-client-01.log`；其 source 后新增提交不属于该包。bec60f86 完成过一次授权真实自我开发，不能称最新产物真实验收。所有首次失败保留。

## 交给用户前写清

- 实际应用路径、冻结 SHA、macOS 14+ 和已安装/登录的 Codex CLI。自动发现 `~/.local/bin/codex`、`/opt/homebrew/bin/codex`、`/usr/local/bin/codex`；其他路径显式使用 `PENDINGCREW_CODEX_PROGRAM`。
- 默认独立根 `~/Library/Application Support/PendingCrewNext`，可用 `PENDINGCREW_CREWD_DATA_DIR` 指定；不与旧根共用，不自动导入旧库。启动前确认该根已有实例和数据归属。
- 打开首版后建机组、选工作目录、交代初始任务；后续群消息通知选中成员。错误显示在窗口，后台日志在 `<独立根>/client-backend.log`。
- 退出窗口不等于停止后台/成员。先在窗口停止成员；需要退出后台时核对对应数据根和自有 PID，通过已有 crewd 控制入口操作，不使用广泛 kill 命令。后台重启后人工选择恢复/暂缓，核对中断工作的实际结果。
- 旧应用与旧数据继续保留。当前首版是操作窗口；用户约定的微信式通讯录/消息与 Todo/文档/进展仍由前端设计线推进，不把操作窗口称作最终界面完成。

安装、正式发布、日常数据切换与追加真实付费任务按既有授权分别执行。可先完成构建、合成验收和交接材料，再提出具体可审阅的授权动作。实际安装/迁移未验收时明确写“未完成”，不由源码测试推断。
