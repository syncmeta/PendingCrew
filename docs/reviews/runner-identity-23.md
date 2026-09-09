# #23 成员恢复 runner 身份

基线：c3e37e3。当前基线的 restartMember 已先读账本并优先使用有效 agent kind；并非仍是 8 月 27 日描述的原始实现。历史引入提交由 `git log -S 'let recordedKind'` 定位到 bf07de8。仍存在无记录/无效 kind 静默 fallback。

## 修改前 inferred 调用清单

使用 `rg -n 'inferred\(fromDisplayName' Sources Tests` 全仓核对：5 个调用表达式，分布在 3 个调用函数中（另有 1 处定义，不算调用）。

1. Sources/Mac/Services/CrewSessionRunner.swift:2522 — restartMember：有效账本优先；无记录或无效 kind 才拿显示名当恢复依据，失败后静默用 captainDefault。此次修复范围。
2. Tests/PendingCrewTests/CrewLocalMentionInjectLogicTests.swift:272 — testKindInferredFromMemberDisplayName：Claude Code 显示名推断单测，不参与生产恢复。
3. Tests/PendingCrewTests/CrewLocalMentionInjectLogicTests.swift:273 — 同上，Codex 推断单测。
4. Tests/PendingCrewTests/CrewLocalMentionInjectLogicTests.swift:274 — 同上，未知显示名返回 nil 单测。
5. Tests/PendingCrewTests/SessionConfigTests.swift:83 — testTerminalHasNoAgentOrServerRunnerSemantics，不参与生产恢复。

定义：Sources/Mac/LocalRunner/LocalCodingAgentKind.swift:63，hasPrefix 实现在 :65。

## 验证状态

先把现有生产决策原样提取到 LocalCodingAgentKind.restartingMember，再测实际写入隔离账本的自定义名称 Codex 和无记录自定义名称成员。无 GUI、无真实 runner 启动。

首次 xcodebuild 在下载依赖时失败（DNS），Executed N 不存在，不计为红测试。完整日志：.test-archive/identity23/baseline-red.log。

修复前实测：baseline-red-retry.log / baseline-results 下 xcresult，Executed 2 tests, 1 failure。
- testRestartCustomNamedMemberPreservesRecordedCodex：通过，验证当前基线已有保护。
- testRestartLegacyCustomNamedMemberFailsLoud：失败，XCTAssertThrowsError did not throw。

修复后首测：green.log / green.xcresult，Executed 28 tests, 0 failures。含 6 条新增恢复行为测试、SessionConfig 其余测试与既有恢复接线测试。

## 范围与边界

- 实测的是生产恢复决策函数、隔离磁盘账本、源码接线；未启动真实 Claude/Codex 子进程，未进行 GUI 恢复操作，因此不声称端到端会话重建实测。
- runner 记录中的有效 `claude_code`/`codex` 按记录恢复，即便显示名故意写成另一种 runner。无效字符串和 terminal 明确失败。
- 无记录且显示名不能识别：明确停止恢复；无记录但显示名仍以 Claude Code/Codex 开头：保留旧推断。因此旧成员若恰好取了另一种 runner 的前缀，仍可能猜错。此次未更改 inferred 的 hasPrefix 规则。
- 捕获账本现有 onIncident 信号：整文件损坏、读取失败不能当无记录继续。未修改 MultiProcessJSONStore 的逐条容错策略；混合数组中某一条结构损坏被丢弃、其他行仍能解码时，底层目前不报告该行事件，恢复入口仍可能把该条视为无记录。磁盘文件被删除、历史记录从未写入，也没有可靠身份可恢复。以上是现有存储层边界，不声称此次已解决。
- 持久化记录内容本身若有效但错误，本次不会通过显示名推翻它；假设写入端记录的 runner 是正确的。
- 没改共享主树、生成物、提示词或 .wrangler；没有新增 Swift 文件。未 push、未合 main。

## 独立变异自证

两趟均只运行指定两条测试，均 Executed 2 tests, 1 failure；均保留完整 log 和同名 xcresult。

- mutation-recorded：只把有效记录分支的 `return kind` 改为 `inferred(...) ?? .claudeCode`（模拟默认 Claude 的历史现场）。Codex 测试明确失败：实际 claudeCode != codex；旧数据 fail-loud 测试仍通过。记录有效性 guard 成功通过，没有更早短路替这条测试挡住错误。
- mutation-legacy：先恢复记录分支，再只把 unknownLegacyKind throw 改成 `return .claudeCode`。无记录测试明确失败：did not throw；持久 Codex 测试仍通过。两项撤回分开执行，彼此不遮挡。

两项均已恢复修复版本；最终全量验证待跑。

## 修改后调用清单

仍为 5 个 inferred 调用表达式，归属 3 个函数：
1. Sources/Mac/LocalRunner/LocalCodingAgentKind.swift:89 — restartingMember（唯一生产调用，只有无记录且无读取异常才进入）。
2. Tests/PendingCrewTests/CrewLocalMentionInjectLogicTests.swift:272 — testKindInferredFromMemberDisplayName。
3. Tests/PendingCrewTests/CrewLocalMentionInjectLogicTests.swift:273 — testKindInferredFromMemberDisplayName。
4. Tests/PendingCrewTests/CrewLocalMentionInjectLogicTests.swift:274 — testKindInferredFromMemberDisplayName。
5. Tests/PendingCrewTests/SessionConfigTests.swift:147 — testTerminalHasNoAgentOrServerRunnerSemantics。

restartMember 的生产接线位于 Sources/Mac/Services/CrewSessionRunner.swift:2523，传入账本 kind 及读取诊断；launchWorker 使用该返回 kind。接线单测限制检查到 restartMember 函数体内；不是仅在整个文件找一个词就判绿。
