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

### 1. `project.yml` 是工程定义的唯一真值，改完必须 `xcodegen` 并提交 `.pbxproj`

```sh
xcodegen        # 改了 project.yml 之后
```

`PendingCrew.xcodeproj/project.pbxproj` 虽然被 git 跟踪，但它是**生成物**，不要
手改。跟踪它是为了让 clone 下来的人不装 XcodeGen 也能直接开工程。

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

### 3. 绕过约束的临时方案要留痕

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
Config/Signing.xcconfig        签名默认值（ad-hoc）；本机覆盖写 Config/Local.xcconfig
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
