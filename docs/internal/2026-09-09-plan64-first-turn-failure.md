# 计划 #64：首回合失败不能显示空闲

基线：`784e3ae`。本单开工时，`9fd6f74`（Todo #117）已经包含原始 400 的产品修复；本单不把别人的修复重做或记成自己的交付。以下行号对应该基线。

## 错误实际走到哪里

1. `Sources/Mac/LocalRunner/CodexAppServer/CodexAppServerConnection.swift:116-123`：stdout 按行解 JSON RPC，经 dispatcher 进入通知序列。这里的 `try?` 会吞畸形 RPC，但**有效 JSON 的本次错误不在此丢失**。同文件 `:100-102` stderr 仅排空；仅存在 stderr 的错误另属未覆盖边界。
2. `Sources/Mac/LocalRunner/CodexAppServer/CodexAppServerBackend.swift:132-138`：按序消费通知；`:337-348` 先应用 transcript，再 trackTurn，调用 `CodexProtocol.sessionHealth` 翻 health。
3. **历史丢弃点**：`9fd6f74^:Sources/Mac/LocalRunner/CodexAppServer/CodexProtocol.swift:227`，`guard let error, let code = error["codexErrorInfo"] as? String else { return nil }`。对象型 `httpConnectionFailed` 被压成 nil；即使是字符串，`:241-242` 未识别类型也返回 nil。错误正文已经到达解析器，但不再进入 health。
4. 后端 `:357-367` 在回合结束清掉 active turn / isWorking；app-server 进程仍 running。`Sources/Mac/LocalRunner/SessionHealth.swift:78-83` 在无 health、running、非 working 时返回 `idle`。因此故障不是“进程退出被当成功”，而是“进程活着，回合失败被丢掉”。
5. 现有修复 `CodexProtocol.swift:227-259` 为缺详情失败、版本不兼容、未知终局错误提供 health；后端 `:310-315` 还处理 `turn/start` RPC 被拒。已有 health 出口通过 `SessionProtocolEndpoints.swift:250-252` 发布、`RemoteSessionBackend.swift:399-403` 接收。`SessionStatusDot.swift:72-73` 把 `error` 显示为红色 attention。`CrewSessionWindowView.swift:1108-1112` 实际从 run 取上述输入并使用该状态点推导。
6. `CrewSessionRunner.swift:2966-2977` 观察 health，`:3019-3047` 首报到白板。同类错误按 kind 去重；镜像不重复发。测试目前选成员状态出口，不以源码调用冒充白板落盘实测。

## 为什么不改 hasObservedLaunchSignal

`SessionBackend.swift:110-125` 明确要求五个后端各答真实启动活迹，没有默认实现；Codex `CodexAppServerBackend.swift:86-89` 以取得 thread id 为据。首回合失败时握手已经成功，改变这个判据会把“回合被拒”误报为“没有启动”。直接复用协议已经要求实现的 `health` / `healthPublisher`（同文件 `:95-96`），不增加另一种生命周期状态。

已读 `537c274`：它统一 viewer 主动断链清理并结束在途请求，不负责回合失败分类。本单不重复其机制。

## 验证设计与记录

`CodexFirstTurnFailureTests` 放在已有 `CodexProtocolTests.swift`，源码接线断言放在已有 `ViewWiringTests.swift`；没有新增 Swift 文件或改生成物。以本机 Python 替身说真实 JSON RPC：握手、建 thread、第一轮失败，进程保持存活。实际走管道解码、dispatcher、通知序列、Codex 后端、协议状态发布、remote，再断言成员状态为 error、状态点为 attention、原始错误仍在。

三条 fixture 分开：只有 failed `turn/completed`（之前仅 started）、只有终局 `error`、只有 `turn/start` RPC error。避免同一 fixture 的前置错误先翻 health，掩护后续处理被删。另断言失败不产生成功回合收尾通知。

- 初跑 `.test-archive/64-baseline.log` 因沙箱 DNS 无法下载依赖而退出 74，没有执行测试，不能记为功能红。
- 当前基线 `.test-archive/64-baseline-retry.log`：Executed 3 tests / 0 failures。第一次功能造红使用真实历史解析函数（`9fd6f74^`），而非将整个函数随意改为 nil：Executed 3 tests / 24 失败断言，其中本地和远端均明确 `idle != error`。

### 独立变异矩阵

以下每次只改指定层，运行后恢复原字节，再进行下一项。入口 C = failed turn/completed，E = 终局 error，R = turn/start RPC error。每项都有完整同名 `.test-archive/64-<名称>.log` 和 `.xcresult`；失败数是 XCTest 断言数，不是失败用例数。

| 名称 | 临时变异 | C / E / R | Executed / 失败断言 |
| --- | --- | --- | --- |
| historical-parser | 换回 `9fd6f74^` 的 sessionHealth 函数 | 红 / 红 / 红 | 3 / 24 |
| backend-notification | `health = detected` 改为 `_ = detected` | 红 / 红 / 绿 | 3 / 16 |
| backend-rpc | RPC catch 内 `health = sessionHealth(...)` 改为只调用不赋值 | 绿 / 绿 / 红 | 3 / 8 |
| state-publish | healthPublisher 的 delta.health 固定 nil | 绿 / 红 / 红 | 3 / 8 |
| remote-health | 接收 state 后固定 health=nil | 红 / 红 / 红 | 3 / 12 |
| member-state | 移除 state 推导中非 launchFailed 的 health 分支 | 红 / 红 / 红 | 3 / 12 |

`state-publish` 的 C 为绿不是漏记：`SessionProtocolEndpoints.swift:543` 每次发任何状态都会重新 `makeState`，`:575` 又从后端读 health。C 在 health 发布之后还有 isWorking=false，后一份快照把错误补回来。E/R fixture 没有后续 working 更新，因此能独立抓住专门 health 发布失效。不能只跑 C 就声称这一层受保护。

六项每次 xcodebuild 均退出 65，均确实执行 3 条测试；源码已逐项恢复。没有把编译错误或测试进程崩溃算成变异成功。

尝试补 `CrewSessionRun` 动态观察断言时，发现 standalone 测试 target 不编译这个类型，`.test-archive/64-run-green.log` 是编译失败，**不算红测**；该尝试已撤回，没有扩大测试目标依赖。

使用已有 `ViewWiringTests` 范式新增 `testFirstTurnFailureHealthReachesRunAndMemberStatusDot`：直接读取必需源码（失败会抛错，不跳过），去掉行注释后检查观察函数被调用、从 healthPublisher 取值、写入 run.health，以及成员视图从 run.health 调用共享 state/dot 推导。单独对 `CrewSessionRunner.swift:2971` 的 `self.health = h` 做变异。这是**源码接线证据**，不能冒充 run 的动态观察或 SwiftUI 实机验收；重构正确代码可能要求同步更新该检查。变异结果 `.test-archive/64-run-observer.log` / `.xcresult`：动态 3 条全绿，接线 1 条失败，总计 Executed 4 tests / 1 失败断言。错误恰在 run.health 赋值断言。所有产品源码已恢复，提交只含测试和本文。

最终全量在提交后运行，结果以交付消息的确切 SHA、Executed 计数和归档日志为准，不在提交前预报通过；全量归档目录 `.test-archive/full64/`（完整日志和 xcresult）。全量同时覆盖恢复后的三条动态测试与接线测试。

## 边界

- 已发生现场由派单提供；未重放真实服务端 400，未读取该 session 原始 rollout。对象型 error info 是协议 fixture，与历史解析漏洞吻合，不把它宣称成对现场原始包的取证。
- 本单测试基于合法 JSON RPC，Python 为协议替身；没有调用真实模型、升级 CLI、启动 GUI 或验收已安装应用。
- 仅 stderr、畸形 JSON、未来改名的通知、断线后永远没有终局事件不由这组测试证明。健康解析仍依赖当前协议消息结构；旧程序没有升级不会因源码修复而自动改变。
- 普通重试中的错误不应立刻当终局；版本不兼容按现有实现可立即报。成功后恢复及再失败去重另由现有代码处理，本单尚未对群消息落盘或恢复后的再次告警做动态验收。
- remote 通知事件与状态快照异步到达，可存在传播窗口；本组等待有界状态收敛，不声称每一个瞬间都无短暂 idle。
