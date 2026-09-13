# #91 重派现场复核

本次接单基线为 `5f02cb2`，独立工作树起始干净。派单称尚未实现，但现场确认 `5ed062d` 已是 HEAD 祖先，缓存源码与测试相对该提交无差异；已有存储注入、不驱逐字典、立即驱逐 fake 和 8 条测试。不重复改写已完成的实现。

## 已复核的历史证据

直接读取 `/tmp/crew-cache-91-evidence/` 的完整日志及 patch，不只依赖既有报告：旧 6 条测试在立即驱逐存储下有 2 个断言失败（命中、maxPixel）；修复后 8/0；去掉 `storage ??` 后 8 条测试有 4 个断言失败，包含注入字典未收到对象及成本 800/1；cp 恢复后 8/0。历史 detached `5ed062d` 全量为 2607 executed、3 skipped、0 failures。这是历史执行，本次执行另记。

原报告所述 `samples/2026-09-12-local-image-cache/` 不在当前树或 `8a1b256` 的 git tree 中；目前可核验 patch 位于上述临时证据目录。不能把该 samples 路径描述为已经提交的证据。

## 生产调用及边界

`Sources/Mac/Support/CrewLocalImageCache.swift:7` 定义协议；`:14` 的生产适配器包装 NSCache，`:17` 设置成本上限；`:77` 新 storage 参数默认 nil，`:79` 使用生产适配器，`:41` 的 shared 是生产构造入口。非 nil 注入只在测试使用，key、成本公式及解码均未变。

`Sources/Mac/Support/CrewImageLoader.swift:50` 算 key、`:57` 查 shared.peek、命中在 `:60` 返回，因此跳过 `:65` 的后台解码；未命中后在 `:70` shared.store。这是源码接线核查，未做 GUI 解码计数验证。

字典隔离了 NSCache 的驱逐不确定性，保留条目时同 key 返回同对象，原图与缩略图可同时存在。它不保证生产必命中，不验证 NSCache 压力阈值、并发请求去重或 TSan。mtime 与 size 同时未变的覆盖可能仍命中旧内容。PNG 编解码、临时目录权限、磁盘满或进程资源耗尽仍可能导致环境假红。未增加 Swift 文件，不需生成工程。

## 本次验证

干净 detached 验收树 `/tmp/crew-cache-91-recheck-5f02cb2`，由 `git worktree add --detach ... 5f02cb2` 创建，并 `cp -R` 共享 Fixtures；复制后 status 为空。开跑前磁盘可用 15 GiB。全量通过 `scripts/test-mac.sh`，DerivedData 在该树自己的 `.test-archive/dd`。首次尝试被沙箱 DNS 阻断，没有执行测试，不算红；获准后重试。

本轮日志目录 `/tmp/crew-cache-91-recheck-evidence/`。本次已完成：

- 首次实际全量 `full.log`：2781 executed、3 skipped、8 failures，exit 65，缓存 8 条通过。唯一具名失败为 `CodexFirstTurnFailureTests.testFirstCompletedFailureNeverLooksIdle`（8 条断言，backend 和 remote 各 4 条；health nil、state/dot 仍 working）。这是当前基线全量失败，不能以历史绿代替。
- 变异前报告已提交为 `aced692`，被变异实现早已提交于 `5ed062d`，验收树 HEAD 为 `5f02cb2`。当前树先保留源码/测试 cp 备份，再从 `5ed062d^` 取旧测试并注入立即驱逐 fake，原断言重放 `01-old-assertions-red.log`：6 tests、2 failures，exit 65，恰好为任务点名的两条测试。
- 恢复现有测试后仅移除 `storage ??`：`02-ignore-injection-red.log` 为 8 tests、4 failures，exit 65。字典接线断言再次稳定检出对象、成本未写入。
- 通过 cp 还原两文件，`git diff --exit-code` 通过，`03-restored-green.log`：8 tests、0 failures，exit 0。上述操作由 `replay.sh` 执行，含 EXIT trap 再次 cp 还原。
- 全量失败用例定向复查 `04-unrelated-failure-recheck.log`：1 test、0 failures，exit 0。这不足以判定根因；未修改该用例或其生产实现。
- 复跑全量前 status 仍为空，磁盘 14 GiB。第二次全量 `full-recheck.log`：2781 tests、3 skipped、0 failures，182.847 秒，exit 0；逐条为 2778 passed + 3 skipped。跳过为 `AgentTuiFixtureRecorder.testRecord`、`CrewLastMessageCacheTests.test_基准_现场白板目录`、`SessionAwaitingReplyInputsCacheTests.test_基准_现场目录`。

最终验收树 HEAD 仍为 `5f02cb29fdd5a9c3eb958b6c5afe74a39c2e08ae`，status 为空。两次全量的日志与 xcresult 均拷入独立证据目录后，自有 `.test-archive` 已删除，磁盘恢复 15 GiB。没有 push、没有 merge、没有修改生产或测试最终源码；本轮交付仅是复核报告与重放证据。

第二次全量绿不能消除第一次红：Codex 首轮失败状态用例在首轮全量的 8 秒等待后 health 仍为 nil，单测和第二次全量通过；原因未查明，不宣称缓存修复解决了它。后续若跟进应另立该用例的调查，不把此次缓存修复回退。

可随提交审阅的重放脚本、两份 patch、红绿摘要及完整日志 SHA-256 位于同目录 `2026-09-13-local-image-cache-recheck-evidence/`。完整日志和 xcresult 留在 `/tmp/crew-cache-91-recheck-evidence/`，临时路径不承诺长期保留。

## 机长要求的补充复核

首轮全量失败断言原文（8 条，未经去重）：

```text
/tmp/crew-cache-91-recheck-5f02cb2/Tests/PendingCrewTests/CodexProtocolTests.swift:484: error: -[PendingCrewTests.CodexFirstTurnFailureTests testFirstCompletedFailureNeverLooksIdle] : XCTAssertEqual failed: ("nil") is not equal to ("Optional(PendingCrewTests.CrewSessionHealth.Kind.cliVersionIncompatible)")
/tmp/crew-cache-91-recheck-5f02cb2/Tests/PendingCrewTests/CodexProtocolTests.swift:485: error: -[PendingCrewTests.CodexFirstTurnFailureTests testFirstCompletedFailureNeverLooksIdle] : XCTAssertTrue failed
/tmp/crew-cache-91-recheck-5f02cb2/Tests/PendingCrewTests/CodexProtocolTests.swift:488: error: -[PendingCrewTests.CodexFirstTurnFailureTests testFirstCompletedFailureNeverLooksIdle] : XCTAssertEqual failed: ("working") is not equal to ("error") - First-turn failure must not appear as idle
/tmp/crew-cache-91-recheck-5f02cb2/Tests/PendingCrewTests/CodexProtocolTests.swift:489: error: -[PendingCrewTests.CodexFirstTurnFailureTests testFirstCompletedFailureNeverLooksIdle] : XCTAssertEqual failed: ("Optional(PendingCrewTests.SessionStatusDot.working)") is not equal to ("Optional(PendingCrewTests.SessionStatusDot.attention)")
/tmp/crew-cache-91-recheck-5f02cb2/Tests/PendingCrewTests/CodexProtocolTests.swift:484: error: -[PendingCrewTests.CodexFirstTurnFailureTests testFirstCompletedFailureNeverLooksIdle] : XCTAssertEqual failed: ("nil") is not equal to ("Optional(PendingCrewTests.CrewSessionHealth.Kind.cliVersionIncompatible)")
/tmp/crew-cache-91-recheck-5f02cb2/Tests/PendingCrewTests/CodexProtocolTests.swift:485: error: -[PendingCrewTests.CodexFirstTurnFailureTests testFirstCompletedFailureNeverLooksIdle] : XCTAssertTrue failed
/tmp/crew-cache-91-recheck-5f02cb2/Tests/PendingCrewTests/CodexProtocolTests.swift:488: error: -[PendingCrewTests.CodexFirstTurnFailureTests testFirstCompletedFailureNeverLooksIdle] : XCTAssertEqual failed: ("working") is not equal to ("error") - First-turn failure must not appear as idle
/tmp/crew-cache-91-recheck-5f02cb2/Tests/PendingCrewTests/CodexProtocolTests.swift:489: error: -[PendingCrewTests.CodexFirstTurnFailureTests testFirstCompletedFailureNeverLooksIdle] : XCTAssertEqual failed: ("Optional(PendingCrewTests.SessionStatusDot.working)") is not equal to ("Optional(PendingCrewTests.SessionStatusDot.attention)")
```

补跑范围是整个 `CodexFirstTurnFailureTests` 套件（3 条），区别于此前只跑失败的 1 条用例。仍使用干净 detached `5f02cb2`，新建该树自有 `.test-archive/dd`；未修改测试或生产源码。两次顺序执行，完整日志为 `05-codex-suite-1.log` 与 `05-codex-suite-2.log`，两次均 exit 0、Executed 3 tests / 0 failures，分别 0.108 秒和 0.082 秒。与机长提供的两次 3/0 对照读数一致。

结论限于：首轮全量失败未在这两次单独套件运行中复现，且此前第二次全量也通过。不能仅凭这些样本确定执行顺序是根因，尚需区分共享状态、时序与资源因素；本轮不修。
