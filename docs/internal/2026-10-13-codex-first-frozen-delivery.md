# Codex 首版冻结交付检查点

## 当前冻结包（取代下方早期检查点）

当前运行源码 fe9ef80e86c86206106cd6c164662d30a1727fe0；应用 /private/tmp/pendingcrew-codex-delivery-recovery-20261013/PendingCrew Codex First.app。此版本补齐握手错误保留、有界重连、旧连接代际保护、终止状态下实际回执排空及休眠初始化保护。后续提交只扩验收脚本/文档，运行 Sources/Mac/Crewd 无差异；不把这些后续 HEAD 冒称包内源码。

直接诊断证明：旧版本 id128 的成功休眠回执和 shutdown 已到 Network receive 回调，但状态失败抢先清掉 generation/pending，使客户端丢弃该真实回执。仅改 FIFO 仍失败，最终修复让状态/发送失败统一等待在途读取排空，终止后拒绝新增调用；只有完整且匹配的真实响应才确认成功。首次失败及两次诊断 /private/tmp/pcw-first-delivery-packaged-client-{01,trace-01,trace-02,fixed-01}.log 均保留；不是把正常退出伪作 RPC 成功。

最终证据（均为独立合成数据，假 runner，未调用真实模型）：

- build /private/tmp/pcw-first-delivery-recovery-build-01.log；artifact check /private/tmp/pcw-first-delivery-recovery-artifact-01.log。包旁 source-head/empty patch/hashes 齐全。
- 最终 Swift 四文件 typecheck /private/tmp/pcw-first-delivery-final-swift-typecheck-01.log；同一 package helper 消息、原生审批/输入、旧答复拒绝、重连、休眠 /private/tmp/pcw-first-delivery-recovery-packaged-client-01.log。
- 同一 package helper 正常/SIGKILL 重启 /private/tmp/pcw-first-delivery-recovery-packaged-{graceful,crash}-01.log：人工 later/resume、原 thread、初始任务无重放、已人停成员不复活。
- 六类真实 Swift/Unix peer 状态模型回归 /private/tmp/pcw-client-failure-all-six-final-04.log：后台启动失败、协议不兼容、有界重试耗尽→显式恢复→再次断线自动恢复、完整最终回执、无回执 EOF、截断尾帧。每场景消息只发一次。专用 test-codex-first-failure.sh 可复用；不强迫 Network 调度顺序，不宣称穷尽代际竞态。
- 休眠五阶段 /private/tmp/pcw-first-delivery-shutdown-final-{crew.list,ask.list,restart.pending,session.list,channel.list}-01.log 通过；原窗口草稿验收仍只归属于下方 c230 的测试副本，新包没有追加 app 控制操作。

首版基本使用与恢复链已达到本轮离线组合验收关口。当前未安装、发布、切日常数据或对新包启动真实账户任务；完整微信前端仍未实现。用户当前不急交接，保留此基线，继续整体工程。下方记录保留为早期产物与首次验收历史。

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
