# 全量跑出一条红，单跑不复现 —— 记成「一次未复现的红」

基准提交：`4f45696`（跑的时候 HEAD 就在这儿；工作树带 `-dirty`，脏的只有
`packaging/homebrew/Casks/pendingcrew.rb` 和几份未跟踪的 docs，**没有一个 Swift 文件**，
所以这条红不是谁的未提交改动引起的）。

## 读数

| | |
| --- | --- |
| 全量 | 执行 2781 条，跳过 6 条，`2 failures` |
| **逐条数 `^Test Case .* failed`** | **1 条** |
| 红的那条 | `CaptainLaunchReadinessTests.testEveryClaudeBackendShapeAnswersTheSameSignal` |
| 断言 | 两个后端形态各一条：「`HeadlessSessionBackend` / `AgentTerminalSession` 吐过字节了，自检必须认得出来」 |
| 单独重跑同一个类 | **9 条全过，0 failures**（01:11） |

⚠️ 汇总行那个 `2` 是**断言数**，不是用例数。这两个数在这一趟正好不相等，
所以它当场暴露了；相等的时候最危险，因为它会给错的数法发一张合格证。

## 判定

**一次未复现的红。** 不是「飘」（那是一个已经观察到规律的结论，这里只有一个样本），
也不是「已修」（没有人动过任何东西）。

它测的东西要**真起进程、等对方吐字节**，带超时。我这一趟是在机器同时干着别的活
的时候跑的。所以「负载相关」是个合理的怀疑，但**我没有证据**，不写成结论。

被测实现 `CaptainLaunchReadiness.observedLaunchSignal` 最后一次改动是 09-06 19:11
（`adc2503`，就在那条测试立红之后 7 分钟），之后没动过。

## 证据在哪

`scripts/test-mac.sh` 的归档机制这次起作用了：失败用例名和完整日志都留下来了
（`.test-archive/20260912T163547Z-4f45696-dirty.log`）。那个脚本存在的理由正是
2026-09-07 那次「只 grep 了汇总行、xcresult 被轮转掉、失败用例名永远拿不到」。

**归档我清掉了**（1.2 GB，磁盘只剩 16 GB），所以下次谁再碰到这条，
手上只有这份文档里的名字和断言原文 —— 那正是当时最缺的两样。
