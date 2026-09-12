# `quietLayouts` 这把尺子归零了，而只有那条负向对照在喊

<!-- doc-ref-base: 2ac882e -->
- 基准提交：`d264459`（main）
- 测于：2026-09-12 01:10–01:14，同一台机器连跑 5 趟
- 状态：**main 上现在有一条红**，不是谁刚改出来的

---

## 读数

`LayoutLoopRegressionTests` 整类跑一趟（`Executed 5 tests, with 1 failure`）：

| 用例 | 结果 |
|---|---|
| `testSwiftUIRepeatForeverInAnchoredScrollViewSelfExcites` | ❌ **红** |
| `testBreathingDotDoesNotSelfExcite` | ✅ 绿 |
| `testBreathingSymbolDoesNotSelfExcite` | ✅ 绿 |
| `testTypingDotsLayerViewDoesNotSelfExcite` | ✅ 绿 |
| `testSwiftUIRepeatForeverInLazyListSelfExcites` | ✅ 绿 |

而同一趟里，`quietLayouts` 打出来的计数是：

```
[quietLayouts] 安静窗口内 layout() 次数 = 0
[quietLayouts] 安静窗口内 layout() 次数 = 0
[quietLayouts] 安静窗口内 layout() 次数 = 0
```

**三次调用，全是 0。**

## 这意味着什么

`quietLayouts` 的几个调用方分两种断言：

- 「不许自激」那几条：`XCTAssertLessThan(n, 10)` —— **0 满足**，所以它们**在这种状态下
  必然通过，跟被测对象是什么无关**。它们此刻量不到任何东西。
- 那条负向对照：`XCTAssertGreaterThan(n, 1000)` —— 0 不满足，**红**。

所以现在 main 上的局面是：

> **那条红不是坏消息，它是这一屏上唯一还在工作的部件。**
> 三次 0 里，是它把「这把尺子已经不量东西了」喊了出来；
> 而靠同一把尺子的那几条防护，正安静地绿着。

被架空的那几条防的是 **2026-07-26 17:24 那次布局自激闪退**
（见 `BreathingDotView.swift` 顶部）。它们现在**挡不住那个 bug 回来**。

## 不是环境问题（有对照）

同一趟里 `testSwiftUIRepeatForeverInLazyListSelfExcites` **绿**，而它要求
`n > 1000`，用的是**另一个**取数函数 `quietTodoLayouts`。
**它拿得到几万次，说明窗口服务器和 AppKit 的显示周期这一趟是活的。**

⇒ 归零的是 `quietLayouts` 这一个取数路径（`ScrollView` + 滚动锚点那套骨架），
不是整台机器、也不是「跑测试时没有 GUI」。

## 不是谁刚改出来的

- `Tests/PendingCrewTests/LayoutLoopRegressionTests.swift` 自 `a61e7a5`
  （initial import）起没有任何提交动过。
- 在 main（`d264459`）和一个只改了 `CockpitPlanStore` 的分支上各跑，**4/4 全红**，
  读数逐字相同。
- 同一台机器 40 分钟前的一趟全量是 `2664 / 0 failures` —— 也就是说
  **它是今晚某个时刻开始红的**，而那之间 main 上落的都是 docs 和 MCP 层的提交。
  我没有把那一段逐个二分，这条留给接手的人。

## 给接手的人：**先别动那个阈值**

把 `> 1000` 调小、或者把那条负向对照删掉，是最省事的「修法」，
而它会**把唯一还在报信的部件也关掉** —— 剩下三条继续绿着，防线一条不剩。

要查的是**为什么 `quietLayouts` 变成 0**：那套骨架里的 `ScrollView` 现在还有没有在
布局。两条线索：

1. `quietTodoLayouts` 同一趟拿得到几万次 —— 两个取数函数的差别就是入口。
2. `quietLayouts` 里那扇窗开在 `(-20000, -20000)`、`.borderless`、
   `orderFrontRegardless()`。如果 AppKit / SwiftUI 某次更新开始对完全离屏的窗口
   跳过显示周期，表现就正好是这个。**这是假设，我没有证实。**

## 边界

- 我只跑了这一个类，**没有查其它用同类离屏窗口手法的测试是不是也归零了**。
  这一句本身就是下一个该量的东西。
- 「今晚某个时刻开始红」是从两个读数之间推的，**没有二分定位到具体哪一笔**。
