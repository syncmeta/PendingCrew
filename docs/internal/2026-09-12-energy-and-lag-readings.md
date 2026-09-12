# 能耗大 / 打字卡（Todo #111 与 #140）：一次带边界的读数

基准提交：`6e026b7`（main）。取样时刻 2026-09-12 22:1x，本机当时有 7 个
claude session 在跑、若干 crew 窗口开着。**本文只报读数和它的边界，不给修法。**

---

## 1. 谁在烧 CPU（两点取样，120 秒）

量法：头尾各读一次 `ps -Ao pid,time`，取 CPU 时间**增量**。
不用 `top` 连续采样 —— 那会把被测进程自己叫醒（2026-09-12 早些时候栽过一次：
报「100% 空转」，三次采样 50.4 / 1.5 / 1.0，是采样本身把它唤醒的）。

窗口内全机累计 CPU 时间 156.3 秒 = **1.3 个核满载**。

| | CPU 秒 | 占一个核 |
|---|---|---|
| WindowServer | 53.2 | 44% |
| PendingCrew（GUI） | 19.9 | 17% |
| PendingCrew（daemon） | 14.7 | 12% |
| claude（7 个进程合计） | ~16.6 | 14% |
| 其它（Podcast / Claude Helper / PendingNet / sing-box …） | 余下 | |

⚠️ **没有基线。** 「PendingCrew 不跑的时候 WindowServer 是多少」这台机器上
从没量过，所以**不能**把 44% 说成是我们造成的。要分开只有一条路：
人把 PendingCrew 的窗口全最小化（或退出）再量一次同样的 120 秒。
这一步要人，agent 不该替他退掉正在跑 7 个 session 的 app。

## 2. GUI 进程那 17% 花在哪（`sample` 5 秒）

**不是在画东西，也不在我们自己的代码里。**

- 主线程忙的那一支是 `CA::Transaction::commit` → SwiftUI `ViewGraph.updateOutputs`
  → `StaticBody.updateValue`，其中 **61 个样本里 58 个在
  `ObservationCenter.invalidate`**。
- 叶子帧直方图里排在两个「空闲/阻塞」之后的全是同一族：
  `Hasher._hash` 39、`Hasher._combine` 26、`AnyKeyPath.hash(into:)` 10、
  `Set<AnyKeyPath>.Iterator`、`ObservationRegistrar.Context.cancel`、
  `libswiftObservation` 里的 `_NativeDictionary._delete` / `_NativeSet._delete`。

也就是说：**每次渲染都在大量地注销/重建 observation 注册项，并为 `AnyKeyPath`
反复做哈希。** 这是「视图图观察了非常多的属性、而且每帧都在失效」的签名。

## 3. 边界（哪几句不能从上面推出来）

- **5 秒一次采样、一个进程、一个时刻。** 没有横向对照（别的时段、别的负载）。
- **没查出是哪个 observable 在失效。** `sample` 只到库函数这一层；
  要指名得加埋点重新构建并替换正在跑的 app —— 那是危险操作（要先冷备份数据
  目录、要人 ⌘Q），不该顺手做。
- **WindowServer 那 44% 未归因**（见 §1）。计划 #89 里那条「WindowServer
  44%~52%，要人在场关窗口才量得开」说的是同一件事，今天仍然没量开。
- **代码里没有任何一处按窗口遮挡状态节流**（全仓 `occlusionState` 0 命中）。
  这是一条**读代码读到的**事实，**不是**「因此它在浪费电」——
  遮挡节流缺席只有在确实有持续渲染时才构成浪费，而那一条正是 §1 没分开的。
- 侧栏那颗呼吸点**不是**嫌疑：只有「黄色 = 有事等人拍板」那一态呼吸，
  而且带数字时走的是静态那一支（`CrewSidebarCrewRow.swift:306-318`）。
  SwiftUI 的 `repeatForever` 早就全换成 CALayer 了。

## 4. 下一步（按代价从小到大）

1. **要人做的那一步**：把 PendingCrew 窗口全最小化，跑一次同样的 120 秒取样，
   对比 WindowServer。这一步不做，§1 那张表就永远说不出归属。
2. 指名那个每帧失效的 observable：要埋点 + 换 app，属重操作。
3. 遮挡节流：等 1 和 2 有结论再谈，现在做等于对着未归因的读数优化。
