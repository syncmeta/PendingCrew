# 父 crew Agent Todo #84：当前 main 的前后端分离复核

- 复核时点：2026-09-27；`main` 为 `45232ec`，tag `v0.1.40`。
- 本文只记录这次在当前树和本机安装版上核到的事实。8 月设计稿的测试数字、旧分支回报均不充当本次测试结果。
- #84 属于父 crew 的 Agent Todo；本组交付证据与缺项，由父 crew 翻牌。

## 当前实现

| 范围 | 当前 main 的依据 | 结论 |
|---|---|---|
| P0 所有权与单实例 | `Sources/Mac/Services/SessionHost.swift`、`Sources/Mac/LocalRunner/ProcessRole.swift`、`Tests/PendingCrewTests/OrchestrationGateTests.swift` | 源码和测试在 main；默认 GUI 角色为 viewer，长期编排归后台。 |
| P1 终端内核与镜像 | `AgentSessionCore.swift`、`TerminalMirrorView.swift`、`AgentTerminalSession.swift` 及 `AgentSessionCoreTests.swift` | 实现与测试在 main；本轮未做人工手感验证。 |
| P2 协议与进程内传输 | `SessionProtocol.swift`、`InProcessTransport.swift`、`RemoteSessionBackend.swift`、`SessionProtocolCodecTests.swift` | 实现与测试在 main。 |
| P3 快照与背压 | `TerminalSnapshotEncoder.swift`、`SessionAttachQueue.swift` 及对应测试和终端 fixture | 实现与测试在 main。 |
| P4 真进程分家 | `SessionDaemonMain.swift`、`UnixSocketTransport.swift`、`SessionDaemonHost.swift`、`ViewerSessionClient.swift`、`OrchestrationGateTests.swift`、`UnixSocketTransportTests.swift` | 实现与测试在 main；本机安装版后台状态探针连通。 |
| P5 现行范围 | `ProcessRole.resolve` 默认 daemon 路径；`MenuBarPanel.swift`、`SessionDaemonStatusMain.swift`、`SessionDaemonStopMain.swift`、`SessionOrphanReaper.swift`、`DaemonIdleReclaimTests.swift` | 默认 daemon、菜单栏、状态/停止、孤儿核验和半开连接回收已有实现。登录自启于 9 月 7 日被人类取消。 |

## 本轮运行与测试

- `/Applications/PendingCrew.app` 的 `CFBundleShortVersionString` 为 `0.1.40`；安装版 `--daemon-status` 返回成功：daemon PID 33627，版本 `0.1.40(20723.43230)`、协议 1、前端连接数 1、运行中 session 10。该读数证明状态接口及当前后台可连接，**不证明**更新或崩溃恢复全链路。
- 最初在 `/tmp` 从 HEAD 的 `git archive` 建隔离树，两次 `xcodebuild test` 均在 Swift 包解析阶段退出 74；`github.com` DNS 失败，执行用例数为 0。后来找到本机 SPM 缓存并在全新 DerivedData 中使用；当前 main 的 `ProcessRoleTests` 7/7、`TodoMarkdownRenderingTests` 19/19 通过。
- 当前 main 的全新构建目录全量 macOS 测试执行 **2931 tests / 4 skips / 1 failure**。唯一失败是 `HelperBuildPerMemberTests.test_真进程_Sparkle把包挪走换上新版_argv路径没变_必须判旧`：换包后真 helper 已退出。该具名用例单独重跑通过（5.021 秒）。所以全量门槛本轮仍记红，不能用单独重跑覆盖原失败。完整日志在 `/tmp/pcw84-current-full.log`，具名重跑在 `/tmp/pcw84-helper-recheck.log`。
- 该真进程夹具随后被确认会触发 macOS 的“App 已损坏”系统弹窗；不再执行它。daemon 独自退出后的恢复提示候选在隔离 worktree 经编译红回归及安全定向 **46/46** 通过，合入本地 main 为 `6e9053e`。合入后用全新 DerivedData、显式 `-skip-testing:PendingCrewTests/HelperBuildPerMemberTests/test_真进程_Sparkle把包挪走换上新版_argv路径没变_必须判旧` 跑 macOS suite：**2944 tests / 4 skips / 0 failures，`TEST SUCCEEDED`**。日志在 `/tmp/pcw84-integrated-safe-full.log`；被跳过的危险用例不计为通过。
- 工作目录原有 `.wrangler/` 与 ACP 文档未跟踪项未动。

## 现行目标与剩余范围

8 月设计稿 A1 要求安装更新时任何 session 都不中断，且覆盖同版、旧版兼容、旧版不兼容三路径。9 月 11 日之后，人类允许后台换代中断 session，再询问是否恢复。当前 `BackendUpdatePlan` 在 build 不同时会换代后台，即使还有 session 在跑；`SessionHost` 接上恢复提示与后台执行。**旧 A1 不能核销为通过，也不应为满足它而回退现行策略。**

尚缺本轮可采信的验收：

1. daemon 独自退出后的恢复提示已在 `6e9053e` 实现并通过当前 main 的安全测试。viewer 在旧后台断链时保存候选和退出印记，新后台握手后只在进程身份确实变化、存在候选且旧后台意外结束或换代时给出恢复提示；恢复仍需用户选择，viewer 不直接拉 session。真 daemon 异常退出后的 GUI 弹窗和点击恢复还没有安装态端到端证据，不能称为实测通过。
2. 安装态更新、重开与恢复的闭环。当前 `/Applications/PendingCrew.app` 和运行中的后台仍是安装版 0.1.40；`6e9053e` 只在本地源码 main，未安装到用户 app，也未 push。源码回归不能替代已安装程序的行为验收。
3. 终端复制、回滚、缩放和未 attach 时主线程开销的验收。图形界面验证需先获人类许可；旧人工验收记录不能冒充本轮实测，也不作为已经落地的 P0–P5 代码核对阻塞。

新增的每 PID daemon 退出印记目前没有自动清理，长期频繁重启会积累小型 JSON 文件；清理时须保留当前与上一轮仍可能被 viewer 判定的记录。#84 的代码缺口已补，父 crew 可凭本页核销本地源码阶段；安装态和界面验收仍应作为独立边界保留。
