# Codex 首版冻结交付检查点

本轮从已接受 main 派生 codex/codex-first-delivery-20261013，只纳入客户端草稿隔离、工程源引用及交付防错。完整工作跟进与巡检仍在 codex/work-identity-20261013；不会因首版冻结丢弃这些工程。

## 产物与证据

- 冻结运行源码 c230a8f51a1a611c3a087f1f5116b45f882fc5f3。
- 独立应用 /private/tmp/pendingcrew-codex-delivery-20261013/PendingCrew Codex First.app。
- 构建日志 /private/tmp/pcw-first-delivery-build-01.log；同目录 source-head.txt、空 source-changes.patch、artifact-sha256.txt。此后本记录的文档提交不在产物内，不混称其为构建 SHA。
- 签名与记录哈希现场核查通过：pcw-first-delivery-artifact-final-01.log。
- 同一包内 release helper + 实际 Swift 客户端 + 假 Codex 的 graceful/crash 重启通过：pcw-first-delivery-packaged-restart-{graceful,crash}-01.log。证明原 thread、人工暂缓/恢复、不重放初始任务、人停成员不复活。
- 草稿模型与客户端合成回归证据见 /private/tmp/pcw-client-delivery-audit-20261013.md。独立读审确认 context/snapshot/revision 的逻辑；已按固定 XcodeGen 2.46.0 重新生成完整工程并确认新源在 Mac Sources 中。首次系统生成器不匹配立即停止，保留 pcw-first-delivery-project-generation-01.log；使用既有缓存后 generation-02.log 通过。
- 实际原生窗口补验：独立合成 HOME/数据库，两机组分别写 A_TASK_DRAFT/B_TASK_DRAFT，切换后各自保留；未点击任务启动、未使用真实模型。测试复制应用仅修改 bundle ID 并重新 ad-hoc 签名以隔离已存在窗口，运行代码来自本冻结包。证据目录 /private/tmp/pcw-first-delivery-gui-drafts-20261013-02；自有 GUI/后台均已收尾。首次沙箱运行 EPERM 保留在无 -02 的目录。AX 行点击未改变选择，改用窗口截图对应行位置后验证，不能把第一次点击宣称成功。

## 尚未证明

此新包没有新增真实账户任务、安装、正式签名/公证、旧数据导入或日常数据切换。历史 bec60f86 的真实开发任务仍只属于当时版本；稳定 main 的原生人工请求/恢复等历史证据按 owning 候选记录引用。本次实际窗口只补草稿跨机组切换，不宣称重做全部窗口矩阵、群/会话草稿或异步 RPC 交互。

最终微信形态、Todo/文档/进展仍为后续客户端工程；本包是首版操作窗口。[具体使用与回退步骤](2026-10-13-codex-first-delivery-checklist.md)。后续先补真正影响使用的组合缺口再交接，不将完整 M4b/行政/其他平台误当首版前置。

## 人类最新节奏（2026-10-13）

用户不急着立即使用，允许继续数晚推进整体。保留首版冻结候选作为运行与验收基线，同时继续整体工程；不承诺完整范围几晚内一定完成。需要 app 使用许可时写清用途与待办，继续不依赖该许可的代码/CLI/离线任务，尽量避免用户离开电脑后整条工程被阻塞。已有权限边界仍保持，不自动安装、发布或切日常数据。
