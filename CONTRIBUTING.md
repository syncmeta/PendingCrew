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

## 三条硬规矩

这三条是被踩出来的，不是审美偏好。

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

### 3. 有些红线文本守卫看不住 —— 别加一条会被调松的

**先看这条红线本身**：白板的读（`LocalWhiteboardStore.list(crewId:)` /
`entries(crewId:)` —— **flock + 整份 JSON 全量解码**）**不许出现在 SwiftUI 的
body 求值路径上**。要时间/末条，取 `CrewStore.lastWhiteboardMessages` 那份后台
按指纹门控算好的快照；本 crew 的完整消息，用视图自己在 `.task` 里订阅的那份。
这条是 2026-08-17「开久了卡」的病根（主线程每秒解析约 11 MB JSON）。

**它在 2026-08-26 又被抓到一次漏网**（`CrewSessionWindowView.latestStep`：
每个 session 行、每次重绘各来一次 flock + 全量解码）。直觉反应是「加条 grep
守卫」。**别加。我们把那条 grep 真跑了一遍，数据否掉了这个方向：**

> `grep "shared\.list(crewId" Sources/Mac/Views/` → **9 行命中，真阳性 0**。
> 8 行是 `LocalTodoStore` / `CockpitPlanStore` 在 `.task(id:)` 里的**合法**读；
> 第 9 行是 `CrewSidebarCrewRow` 里那段记录病根的**注释本身**。
> 而唯一真的那条**一次都没抓到**（它写成了跨行：`LocalWhiteboardStore.shared`
> 换行再 `.list(crewId:)`；跨行调用在本仓是常见写法，不是孤例）。
> —— 数字由「机组群聊体验」那条线实测得出。

**根因不是正则写得不够好，换个更聪明的正则也不行**：合法读和违规读**文本上
完全相同**，区别只在这行代码出现在 `body` 里还是在 `.task` 里 —— 任何正则都
看不见这个区别。更糟的是同一处违规还能以**参数形式**藏起来
（`SessionUnreadStore.unreadCount(..., whiteboard: .shared)`：调用点一个字都不
提那个 store）。

所以，给一条红线加守卫之前，先问：**这条红线的违规特征，是文本能表达的吗？**
不能的话，**宁可老实承认它现在只能靠人看**，也别加一条会响一堆假警报、还把
「这里已经修好了」的注释报成违规的守卫 —— 那种守卫必然被下一个人调松或删掉，
而它留下的「已经有守卫看着了」的假记录，比没有守卫更糟。

真正的出路是结构性的：让违规写不出来（读一律走 `CrewStore`，
`LocalWhiteboardStore.shared` 的读不暴露给 View 层，守门的是类型系统）。
在那之前，这条靠 review 看。

### 4. 绕过约束的临时方案要留痕

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
