# 七条未合分支的去留（2026-09-12 07:45 盘点）

基准提交：`035bfb3`（本次盘点在这棵树上做）。

## 结论

| 分支 | 去留 |
| --- | --- |
| `pendingcrew/session-6ae28d` | **没有东西滞留**，产物全在 main |
| `pendingcrew/session-773939` | 同上 |
| `pendingcrew/session-7755ea` | 同上 |
| `pendingcrew/session-971acc` | **本次已合**（`035bfb3`）—— 唯一真漏掉的一条 |
| `rescue/supervision-lease-107-108` | **没有东西滞留**，产物全在 main |
| `fix/plan-list-unreadable` | **不合**：main 早已用 `LedgerRead` 那对孪生修过，这条是第二份实现 |
| `feat/acp-third-backend` | **不由我处置**：7 笔全新、14 小时前还在动，接不接第三后端是产品级决定 |

**分支一条没删** —— 按仓库惯例，清不清是仓库主人的事。

## 判法：两把便宜的尺子互相打架，都不能单独信

第一把是**「招牌产物在不在 main」**：`add_human_todo`、`SessionOrchestratorLock`、
`SupervisionLease.swift`、`activityRevision` 逐个查，四条分支全中 ⇒ 结论「都进去了」。
**这是个代理量**，它只证明「有个同名的东西在」，不证明这条分支的改动都在。

第二把是 **`git cherry -v main <branch>`**（按 patch-id 找等价提交）。它给出**不一样**
的答案：`session-6ae28d` 有 2 笔、`rescue/...` 有 1 笔标成 `+`（main 上没有等价物）。

**两把尺子打架的时候，谁也别信** —— 去看那三笔各自产出的**文件**在不在 main：

- `Sources/Support/HumanTodoWakePlan.swift` ✅（main 上有 5 个文件引用它）
- `Tests/PendingCrewTests/{HumanTodoWakePlanTests,TodoLedgerIsolationTests,McpAddHumanTodoTests}.swift` ✅
- `Sources/Models/SupervisionLease.swift` ✅（main 上 173 行，那笔加的是 158 行）

**所以 `git cherry` 的 `+` 不等于「没进去」**：这三笔是**重新落的**（rebase / 重提交），
patch-id 对不上而内容在。反过来，招牌产物那把尺子这次**碰巧对了**，但它对得没有道理 ——
它分不出「同名的东西在」和「这条分支的改动在」。

**下一次盘点直接从第三步开始**：拿 `git cherry` 挑出 `+` 的那几笔，逐笔看它的
产出文件在不在 main。前两步都是省不下的那种省。

## 为什么 `session-971acc` 是唯一漏掉的

它的产物 `Tests/PendingCrewTests/CrewCommandDrainLogTests.swift` 在 main 上**不存在** ——
三把尺子在这一条上是一致的。合的过程见 `035bfb3`。
