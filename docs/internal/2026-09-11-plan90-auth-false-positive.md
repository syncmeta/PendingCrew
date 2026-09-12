# 计划 #90：Claude 未登录误报（新单，不修改 #64）

<!-- doc-ref-base: 6cc941e -->
基线 `0d98b93`，独立分支 `pendingcrew/plan90-auth-health`。

## 已核事实

- `Sources/Mac/LocalRunner/SessionHealth.swift:230-274`：`SessionHealthScanner` 对去 ANSI 后滚动尾窗做小写子串匹配，`run /login` / `not logged in` / `invalid api key` 任一个出现即产生 `authRequired` 和可执行的 `/login` 指令。紧邻注释主动承认 cat 到文档会误报；这是已知误报被接受为 warning 的历史取舍，不是未知状态从启动超时推出来。
- `Sources/Mac/LocalRunner/AgentSessionCore.swift:435`：每次 PTY 输出调用 scanner，并把所有 hit 逐个写入 health；不是启动时只检查一次。
- scanner 的 fired 集合按 kind 去重，不会重新确认 auth；`rearmQuota` 只恢复额度扫描。core 的 `QuotaHealthRecovery` 同样只清额度。结果是一次误命中能驻留为长期 authRequired。
- `hasObservedLaunchSignal` 表示收到启动活迹，不回答认证是否有效；把它当“已经登录”会把登录错误本身的输出也算成正常。保留两者职责，不重构生命周期。

## 先造红

新增 `SessionHealthTests.testWorkingSessionQuotingLoginDocumentationIsNotUnauthenticated`：构造 Bash(cat …) 的工具输出，包含源代码 authPhrases，随后正常工作 spinner 和 auto mode；断言不得产生 authRequired。

新增 `SessionHealthTests.testExplicitAuthenticationFailureWithoutLegacyPhraseStillReports`：构造 `API Error: 401` / `authentication_error` / OAuth token expired，故意没有旧短语；断言必须报 authRequired。

原实现运行：Executed 2 tests / 2 failures / TEST FAILED，`.test-archive/plan90/red.log` 和 `red.xcresult`。两方向都实测红，不是只加一个永远返回正常的判定器。

## 修复的判据与更新时机

- `SessionHealth.swift:276` 定义 unknown / authenticated / authenticationRequired，不把未知映射为未登录。未知或正常只清 authRequired，不清其它健康问题。
- `SessionHealth.swift:294` 的只读 probe 按真实 `SessionConfig.resumeSessionId/newSessionId` 找 JSONL，使用目标 env 的 CLAUDE_CONFIG_DIR（相对路径按目标工作目录）或 HOME/.claude；不使用全机 auth status。同 id 多位置、目录/文件读失败、缺失均 unknown。
- `SessionHealth.swift:327-364` 仅解析顶层 runner envelope：同 session、非 sidechain、assistant 类型、本次启动之后的 timestamp。`isApiErrorMessage=true` 且 `error=authentication_failed` 确认失败；非 synthetic 的真实 assistant 消息（model、msg_ id、content 数组）确认本次启动已得到模型响应。普通 user/tool 文本、引用的 JSON 不参与判定。
- 本机已有 JSONL 的**元数据**核对过这两种形状：失败记录 `type=assistant/isApiErrorMessage=true/error=authentication_failed/apiErrorStatus=403/model=<synthetic>`；真实响应 `role=assistant/id=msg_…/model=claude…/content=array`。只提取字段形状，没有复制正文、账户标识或凭据；不是重放任务中那个 worker。
- `AgentSessionCore.swift:153-169` 每 2 秒在 utility Task 读一次有界尾部（最多 256 KiB），主 actor 只更新状态。不依赖 PTY 是否又吐字，因此静止错误/恢复都有复查；`:446-450` 映射 health，`:461` 的旧 PTY scanner 只剩额度。停止或退出后循环检查 running 结束，deinit 取消任务。
- `CrewSessionRunner.swift` 的 health=nil 分支加入 authRequired 首报重新武装，恢复后再次确认失败可以再次公告。没有改变 session 的启动、退出、唤醒或续跑决策；hasObservedLaunchSignal 原样。
- 已更新旧“裸 run /login 必须报警”的测试契约。**原始红样本的 401 也是文本，不能仅凭它就当确认**；最终用例分别断言 PTY401仍未知、同 session结构化 authentication_failed 才报错。后续变异保持新类型和测试可编译，不能把删除类型导致编译失败算成功。

## 验证进展

- `.test-archive/plan90/green1.log`：20 tests / 0 failures。
- `.test-archive/plan90/green2.log`：21 tests / 0 failures，增加真实 AgentSessionCore + 无害 PTY 脚本 + 临时 JSONL 的接线：未知→正常→失败→恢复→再次失败→文件消失回未知。
- `.test-archive/plan90/green3.log`：22 tests / 0 failures，包含有界尾读与不可读文件。
- 修复先提交为 `0236cca`，随后逐项变异；恢复源码后再做固定提交、带 fixture 的全量验收。

## 提交后的变异自证

每项保留同名 `.test-archive/plan90/<name>.log` 与 `.xcresult`；脚本 `mutations.py` 每项 finally 恢复源码，全部退出65且具名断言失败。结束后两个源码文件 `git diff` 为空。

| 变异 | Executed | 断言失败 | 被证实的约束 |
| --- | ---: | ---: | --- |
| restore-both-directions | 2 | 3 | 两条主样本均失败，撤回修复两方向都红 |
| restore-raw-text | 2 | 1 | 单独恢复词表，正常工作样本红 |
| drop-confirmed-failure | 2 | 2 | 单独去掉认证失败分支，真失败样本红 |
| disconnect-core | 1 | 4 | 断开 Core 应用判定，真实 PTY 接线测试红 |
| drop-recovery | 1 | 1 | 删除恢复清理，旧错误驻留被抓到 |
| unknown-as-logged-out | 2 | 2 | 缺日志/不可读不得当未登录 |

这些不是编译失败，Executed 与各项目标数一致。前级误报与后级漏报分别删除，避免早期短路掩盖后级变异。

## 当前边界

- 用户给的 worker 现场作为任务事实；本单尚未取它的原始 PTY 数据，构造样本不声称就是现场逐字重放。
- 仅 PTY 子串无法区分 runner 错误与工具读取的文档；三态不能仅给原 Bool 改名字。
- 当前 Codex 环境中只读运行一次 `claude auth status --json` 得到 loggedIn=false，但这不是那个 Claude worker 的环境/进程，因此不能把全机探针结果直接赋给 session。本次没有打印 token、email 等账户内容，也未运行 /login。
- 不修改旧单 #64、不升级 CLI、不启动图形程序、不修改共享树；后续验收按 detached + fixture + 独立 DerivedData。

## 明确没有保证的范围

- authenticated 的含义是“本次启动观测到过真实模型响应”，不是承诺凭据永远有效；最新确认失败会覆盖它。新启动不把 resume 的旧记录当当前证明。
- Claude 未生成结构化记录（例如还停在登录选择菜单）、日志被拒读/损坏、schema变化、无 session id、custom projectsDirectory 布局、消息长到有界尾部内没有完整当前记录，都会保持未知，可能漏掉认证红点；不能据此说已正常。既有待决策菜单通道未改动。
- 日志是异步落盘，最长约一次 2 秒轮询再加落盘延迟；读取期间的末尾半条等待下一次。每次最多读256 KiB且不在主线程做IO；没有做22+真实session同时轮询的性能实测。
- 信任 runner 写入的顶层元数据，不防有人直接伪造/篡改该 JSONL。仅UI字符串不再被赋予这份信任。
- 旧的正在运行的 backend 不会因合并代码当场换成新实现；没有自动重启用户的session。没有GUI/真实登录故障/群落盘端到端验收。

- 同一 session id 若被两个并发 Claude 进程复用，本次启动时间与 id 无法区分谁写入；本实现不承诺这种场景的归属准确。
- 有效证据之后出现无法解析的完整行时会跳过该行，保留有界尾部内上一条有效证据；若损坏行恰好是相反的新状态，可能暂时保留旧判断。未知不是对凭据有效性的承诺。
