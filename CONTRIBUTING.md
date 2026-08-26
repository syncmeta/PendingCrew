# 参与 PendingCrew

先说清楚期望：这是一个**个人作品**，还在快速变形期，接口和数据格式都会变。
欢迎 issue、欢迎 PR，但请别假设有 SLA —— 我不一定接得住，也不一定接得快。

如果你只是想跑起来看看，看 [README](README.md) 就够了，这份不用读。

---

## 动手之前

**先开一个 issue 说你想干什么。** 尤其是想改结构的时候 —— 这个仓库里有好几处
「看着多余、其实是在填某个具体的坑」的写法，注释里通常写了当时的缘由。先聊一句
能省掉双方各写一遍的功夫。

小修（打字错误、明显的空指针、文档笔误）直接发 PR，不用先问。

**引一条既有结论当依据之前，先核它成立的条件。** 这个仓库里有大量写下了缘由的
注释、`docs/tech-debt.md` 里的条目、还有 commit 说明 —— 它们大多是对的，但**大多
带着一个「如果」**。引用时要核的不是它说了什么，是**它成立的条件在当下还成不成立**。

真踩过一次（2026-08-26，Todo #60）：tech-debt 里写着「**第 4 条要是真出了问题**，
正确的方向是走那条未走的路」，被当成预先授权引用了。转述一字不差、读的人也点头 ——
直到把原文调出来，才看见「第 4 条」指的是另一件事（懒行高度回填之后锚点漂不漂），
而那一条恰恰**通过了**，触发条件根本没成立。结论后来仍然成立，但**理由是错的**，
换了一个才站得住。

**转述无误 ≠ 引用成立。** 一条自洽的话最容易被当成授权 —— 它读起来没有任何破绽，
破绽在它前面那个「如果」里，而那个「如果」通常不在被转述的那一句里。

## 六条硬规矩

这六条是被踩出来的，不是审美偏好。

### 1. `project.yml` 是工程定义的唯一真值，改完必须重新生成并提交 `.pbxproj`

```sh
scripts/gen-project.sh          # 改了 project.yml 之后
scripts/gen-project.sh --fetch  # 本机没装 / 版本不对时，取仓库声明的那一版来用
```

**别直接跑 `xcodegen`。** `.xcodeproj` 是生成物却被提交进仓库，所以**生成它的
那个生成器的版本也是仓库的一部分** —— 版本写在 `.xcodegen-version` 里，CI 装的
就是它。脚本会在生成任何东西**之前**比对版本并停下，免得你先看到一坨看不懂的
pbxproj diff、再花时间怀疑自己是不是忘了 regen（那是同一种症状）。

要升 XcodeGen 版本，就让它是一次**显式提交**：改 `.xcodegen-version` 的数字 +
在 `scripts/xcodegen-checksums.txt` 补一行 + 同一笔里重新生成 pbxproj。这样历史上
「这次 pbxproj 大改是因为换了生成器」是自解释的，而不是某天某人 brew 升级顺手
带进来的。

`PendingCrew.xcodeproj/project.pbxproj` 虽然被 git 跟踪，但它是**生成物**，不要
手改。跟踪它是为了让 clone 下来的人不装 XcodeGen 也能直接开工程。

**合并冲突也一样 —— 生成物的冲突不许手解，重新生成。** 合并/rebase 时 `.pbxproj`
撞了，不要去挑 `<<<<<<<` 两边的行：手解会解出一个**谁都没生成过的中间态**，而它
多半还编得过，于是没人发现它已经和 `project.yml` 对不上 —— 直到某个新 worktree
编不过、或者某个新加的文件莫名其妙不进 target。正确做法是先把 `project.yml` 那边
的冲突解干净（真值在那儿），再 `scripts/gen-project.sh` 重新生成，然后 `git add`
生成结果。这是「`project.yml` 是唯一真值」的直接推论，对任何被跟踪的生成物都成立。

**新增 Swift 文件属于「改了工程定义」** —— 源文件是按目录收的，加了文件不 regen，
你本机能编（Xcode 会自己发现），但别人 clone 下来那份 `.pbxproj` 里没有它，
**在别的机器上编不过**。这条真的踩过。

**CI 会替你查这一条**（`.github/workflows/ci.yml` 的「pbxproj 与 project.yml 同步」，
约 12 秒）。它是唯一一条**你本机永远看不到红**的规矩，所以必须由机器守：
Xcode 会自己发现你新加的文件，于是你能编、能跑、能提交，只有别人 clone 下来
才炸。

### 2. 三端都要编一遍

三端共用一套源码。只编 Mac 会让漏了 `#if os(macOS)` 的 AppKit 调用把 iOS 端
静默打红。

```sh
xcodebuild -project PendingCrew.xcodeproj -scheme PendingCrew \
  -destination 'platform=macOS' build
xcodebuild -project PendingCrew.xcodeproj -scheme PendingCrew \
  -destination 'platform=macOS' test
xcodebuild -project PendingCrew.xcodeproj -scheme PendingCrew \
  -destination 'generic/platform=iOS Simulator' build
```

单测 bundle 只挂 macOS —— 被测代码基本都在 `#if os(macOS)` 后面。

这三条 CI 在 PR 上会跑一遍（冷机约 13 分钟），但**本机先跑更快**：等 CI 告诉你
iOS 端红了，你已经等了十几分钟。

**跑测试要留全日志，别只 grep 汇总行。** 汇总行（`Executed N tests, with M failures`）
告诉你**红了几条**，恰恰不告诉你**红的是哪一条** —— 而**没有名字的红等于没发生过**，
半年后只会变成「这一族偶尔会红」的传说。`docs/tech-debt.md` 里已经躺着一条这样的
旧账（一次全量里见过一个 failure，从头到尾没定位到）。所以：

```sh
xcodebuild ... test > /tmp/test.log 2>&1; grep -E "' failed \(|Executed .* tests, with" /tmp/test.log
```

**还有一条比它更容易骗人的**：**一趟绿是一个样本，不是一个结论。** 这个套件里存在
**只在满载下现形**的竞态用例 —— 单独跑它一百次都绿，全量跑四次能红两次。所以
「我跑过了，绿的」和「这块是干净的」是两句话，改动越靠近并发/时序，两句话之间的
距离越大。真怀疑某条在飘，就**连着跑几趟全量**，别拿一趟绿去放行发布。

### 3. 白板的全量读不许出现在 SwiftUI 的 body 求值路径上

`LocalWhiteboardStore.list(crewId:)` / `entries(crewId:)` 是 **flock + 整份 JSON
全量解码**。要时间/末条，取 `CrewStore.lastWhiteboardMessages` 那份后台按指纹门控
算好的快照；要本 crew 的完整消息，用视图自己在 `.task` 里订阅的那份。

这条是 2026-08-17「开久了卡」的病根（主线程每秒解析约 11 MB JSON）。判据是
**这行代码在 `body` 里还是在 `.task` 里**，不是它写成什么样 —— 同样一行
`list(crewId:)`，在 `.task(id:)` 里合法，在 body 里是违规。

已知未修的一处：`CrewSessionWindowView.badgeCount(for:)` →
`SessionUnreadStore.unreadCount(..., approvals: .shared, whiteboard: .shared)`，
切换条那个 `ForEach` 里每 run 每帧一次、而且是两个账本各一次。它**不是换个快照
就能修的**（未读数要的是「某时刻之后的一段区间」，而那份快照只存末条/计数），
得先给未读数造一份后台产物 —— 归「让 View 层拿不到这个类型」那条结构性改造。

**别给这条红线加文本守卫。** 为什么不能加，见第 5 条第一个实例。

### 4. 目录参数化的 store，别在拿得到实例的地方去够 `.shared`

`LocalWhiteboardStore` / `LocalTodoStore` / `LocalApprovalStore` /
`LocalCrewControlStore` 都是**同一个形状**：既有 `init(directory:)`，又有一个
`static let shared`（吃 `LocalWhiteboardStore.defaultDirectory`）。凡是**自己带着
目录**的调用方 —— MCP helper（靠 `--dir` 定目录）、以及任何注入了 store 的单测 ——
一律用手上那个实例，**不许去够 `.shared`**。踩了不会报错，只会读错账本。

最耐久的防线不是记住哪个 store 吃哪个目录，是**让同一处的几行共用同一个来源**。
`McpServer.blockerState` 是范例：

```swift
agentTodoExists: { todos.item(crewId: crewId, number: $0) != nil },
humanTodoExists: { humanTodos.item(crewId: crewId, number: $0) != nil })
```

两行视觉对称，谁也没法只改一行而不显眼。顺带：**正确的写法通常也是更短的写法**
（`humanTodos` 已由 `sibling(.human)` 默认好，用它不需要任何额外构造）。

例外只有一种：`startWatching()` / `directoryChanged` 这类**变更信号**（不是数据读）
今天确实走 `.shared`。最坏情况是收不到 tick、什么都不刷新 —— 看得见的失败，
不是悄悄读错人。

**这条为什么在 CI 上抓不到，见第 5 条第二个实例。**

### 5. 先证明它会红，再信它的绿

**一把从原理上就看不见你要防的那个东西的尺子，它给出的绿是无内容的。**

所以：**加一把新尺子（守卫、探针、断言、给人念的清单）之前，先让它在一个已知的
坏例子上真的红一次。** 红不出来，说明你要防的东西不在它的量程里 —— 那它以后每
一次绿都不构成证据，而且比没有尺子更糟：它会留下一句「已经有东西看着了」。

这不是三条经验，是同一件事的三种长相。下面三个实例都真跑过，数字都是实测的。
**以后再遇到新的长相，作为例证追加到这一条下面，不要新开一条规矩** —— 新长相是
无穷的，这条判据不是。

#### 实例一：以名字为锚的扫描，看不见「在 body 里还是在 `.task` 里」

第 3 条那条红线，直觉反应是「加条 grep 守卫」。**别加。那条 grep 真跑过：**

> `grep "shared\.list(crewId" Sources/Mac/Views/` → **9 行命中，真阳性 0**。
> 8 行是 `LocalTodoStore` / `CockpitPlanStore` 在 `.task(id:)` 里的**合法**读，
> 第 9 行是 `CrewSidebarCrewRow` 里那段记录病根的**注释本身**。
> 唯一真的那条一次都没抓到。

三层，一层比一层深：

1. **跨行写法** → 单行 grep 失效（漏网那处就是 `LocalWhiteboardStore` 换行再
   `.list(crewId:)`；跨行在本仓是常见写法，不是孤例）。
2. **合法读和违规读文本完全相同** → 区别只在 body 还是 `.task`，任何文本正则都
   看不见这个区别。那 8 个假阳性，每一个都跟违规写法长得一模一样。
3. **依赖以隐式成员传参 → 连类型名都不出现。** `approvals: .shared, whiteboard: .shared`
   靠 Swift 的隐式成员查找补出类型，调用点**连 `LocalWhiteboardStore` 这个词的一个
   字母都没有** —— 不是被换行拆开、不是被别名挡住，是**根本没写**。

第 3 层堵死了这条路。跑过的例子：`CrewSessionWindowView.swift` 修完第一处之后，
在这个文件上扫 `LocalWhiteboardStore` ——

| 扫描方式 | 命中 |
| --- | --- |
| 朴素（不跳注释） | 2（`:497` / `:504`，**都是病史注释**） |
| 跳注释的「聪明」版 | **0** |

而那行 `whiteboard: .shared` 原样跑着。**扫描器越聪明，这个文件看起来越干净** ——
工具每改进一步，谎报得越彻底。真要抓它得做跨函数类型解析，那不是守卫，是编译器。

**这类守卫最坏的形态不是响假警报，是给出零命中。** 假阳性至少说明有人在看，
零命中是直接发一张免检证。出路是结构性的：让违规写不出来（读一律走 `CrewStore`，
`LocalWhiteboardStore.shared` 的读不暴露给 View 层，守门的是类型系统）。在那之前，
这条**老实靠 review 看**。

#### 实例二：CI 上那条用例，从来没读过它该读的目录

第 4 条那种踩法的失败方式比「飘」更坏：

- 生产上 `--dir` 恰好等于 `defaultDirectory`（`LocalSessionLaunch` 传的就是它），
  **写错了生产上看不出来**；
- 单测注入 temp 目录，`.shared` 会绕过 temp 去读**开发者本机真实的那本账**；
- CI 是干净机器、那个文件根本不存在 → **恒定读出「没有」**。

于是**只要断言写的是「没有」，CI 绿、大多数开发机也绿**。这不是 flaky ——
**一个从来没读过 temp 目录的用例，会以一条稳定的绿一直活着**。它在真读发生之前
就已经绿了，所以它的绿跟被测代码无关。

#### 实例三：探针量首尾不量路径，写给人眼的清单也跟着只问首尾

Todo #60（「加载更早」要保持阅读位置）。离屏探针量到：不补偿的对照跳 **680pt**，
补偿后位移 **14pt / 1pt** —— 首尾一致，绿。人类装上更新后的反馈是：

> 加载更早有个很奇怪的 就是 感觉像是又从上面滑下来的感觉 **位置倒是一样**
> 但是直觉上感觉位置不一样

**终点对，路径可见。** 探针量的是起点和终点，从来没量过中间经过哪里。

更值得记的是第二层：`docs/tech-debt.md` 里为这件事专门写了一份「人类装完更新后
照着念」的四条清单，人类真念了、四条都答了，**但他报回来的那个问题，四条里一条
都没问到** —— 四条问的全是「位置对不对」。**清单继承了探针的盲区。**

**人眼是唯一能抓到「过程 / 观感 / 别扭」的仪器，别让它去核对探针已经能答的终点
坐标。** 写给人看的清单，就该只问人才答得出的东西。

### 6. 绕过约束的临时方案要留痕

如果你为了让 A 跑通而把代价转嫁给了 B（关掉某个校验、塞个 placeholder、双写、
用 `#if` 整块屏蔽、只验最容易过的那条路径），**在 `docs/tech-debt.md` 里记一条**，
或者加一道会响的断言。这个仓库靠那本账活着，不靠记性。

写清三件事：为什么这么改、代价转嫁到了哪、失败时长什么样。

## 代码风格

没有 linter，跟着周围的代码写就行。几条本仓库的习惯：

- **注释写「为什么」，不写「是什么」。** 这个仓库注释密度偏高，因为很多地方
  的形状是被具体的坑逼出来的 —— 不写下来，下一个人（包括三个月后的作者）会把它
  「优化」掉，然后重新踩一遍。带日期和现象的注释是资产，不是噪音。
- 中文注释是常态，不用改成英文。
- 主线程很敏感。这个 app 同时挂着 N 个 PTY，往 `@MainActor` 上加同步工作之前
  先想一下它会不会随 session 数线性放大 —— `docs/tech-debt.md` 第一条就是这个。

## 提交与 PR

- 一个提交一件事。提交信息说清**为什么**，别只说改了什么。
- PR 里贴出你跑过的验证（哪几条命令、什么结果）。「应该没问题」不算验证。
- 不要在 PR 里夹带无关的格式化改动。

## 目录速查

```
project.yml             XcodeGen 工程定义（唯一真值）
Config/Config/Signing.xcconfig 签名默认值（ad-hoc）；本机覆盖写 Config/Local.xcconfig
.xcodegen-version       生成 .xcodeproj 用哪一版 XcodeGen（唯一真值）
Sources/
  Mac/                  macOS 专有：LocalRunner（agent 子进程）、Mac 界面
  Mcp/                  crew-comms MCP server —— agent 通过它读写白板、@ 人、请示
  Stores/               本地持久化：白板、Todo、审批、唤醒、crew 树
  Chat/                 群聊 UI
  Remote/               跨端 WS 协议与 viewer 客户端（**尚未接通**，见 README「状态」）
  Views/ Services/ Models/ Support/
Tests/PendingCrewTests/ XCTest（macOS）
Shared/AppUpdate/       Sparkle 自动更新 + 构建版本戳
scripts/                本地小工具 + release/
docs/                   release-macos.md（发版）、tech-debt.md（债）
docs/internal/          开发过程记录，写完即冻结，**不随代码更新**
```

## 有几个测试需要现取 fixture

`CrewChatOpenCostTests` 用的是真实群聊数据（不入版本历史，见 `.gitignore`）。
没有 fixture 时这些用例会 skip 并打出取数据的命令 —— 干净 clone 上那是预期的，
不是失败。要真跑的话：

```sh
scripts/make-chat-fixtures.sh <crew-id>
```

## 安全问题不要开 issue

见 [SECURITY.md](SECURITY.md)。
