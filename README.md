# PendingCrew

在自己的 Mac 上，把多个 coding agent（Claude Code / Codex）当成一个**小组**来带。

**不需要登录，不需要账号，不需要任何后端。** 装上、打开、起 session 就能干活 ——
它拉起的是你本机已经装好的 `claude` / `codex`，烧的是你自己的订阅额度。

每个 agent 是一个 session，它们共享一块群聊白板：互相看得见对方在说什么，可以
@ 点名、可以互相交接、需要人拍板时会把问题发到群里等你回话。你在一个界面里
看全部人的进度，而不是开七个终端窗口来回切。

```
                 ┌──────────────────────────────────────────┐
                 │  PendingCrew.app                         │
                 │                                          │
   你 ───────────┤   群聊白板   ← 所有人都看得见的一块板      │
   （在群里说话）  │      ▲                                   │
                 │      │ 读 / 写 / @ 点名 / 请示             │
                 │      │  （crew-comms MCP server）         │
                 │   ┌──┴───┬────────┬────────┐             │
                 │   │机长   │session │session │  …          │
                 │   └──┬───┴───┬────┴───┬────┘             │
                 └──────┼───────┼────────┼──────────────────┘
                        │       │        │   每个 session = 一个真 PTY
                    ┌───▼───┐┌──▼────┐┌──▼────┐
                    │claude ││codex  ││claude │  ← 你本机装的 CLI
                    └───────┘└───────┘└───────┘
```

**机长（captain）** 是这个 crew 里第一个 session：它读你的意图、把活拆开、
决定该起谁去干、替卡住的成员拍板。你只跟机长说话，它带着其余人跑。

<!-- 截图待补：主界面（群聊 + session 列表 + 终端）。 -->

## 装

到 [Releases](https://github.com/syncmeta/PendingCrew/releases) 下载 `.dmg`，
拖进「应用程序」，打开。已经过 Apple 公证，双击不会被拦。

> 请把它拖进「应用程序」再打开，别直接在「下载」文件夹里双击 —— macOS 会把
> 从下载目录直接运行的 app 挪到一个随机只读路径（App Translocation），自动更新
> 和本地数据目录都会出怪事。

**系统要求**

- macOS 14 (Sonoma) 或更新
- Apple Silicon 或 Intel Mac
- 硬盘上有 [Claude Code](https://claude.com/claude-code) 或
  [Codex CLI](https://developers.openai.com/codex/cli) 中的**至少一个**，
  命令在 `PATH` 上、**且已经登录过**

PendingCrew 自己**不持有任何模型 API key、也不代付任何费用**：它把这两个 CLI
作为子进程拉起来，烧的是你自己的订阅额度。两个都没装的话，app 打得开，但起不了
session。

## 状态

个人作品，开发中，接口和数据格式都还会变。下面这张表是**照实写的**，请按它
判断这东西现在能给你什么。

### 真跑过、天天在用的

这些是这个项目的本体，不需要登录、不需要后端，装上就是全部：

- 建 crew、起 Claude Code / Codex session（共享目录或独立 git worktree）
- 群聊白板 —— session 之间互相看见、@ 点名、交接、把问题抛给你拍板
- 每个 session 一个真 PTY 终端，可以直接接管键盘
- 机长自主派活、起子 session、拆子 crew
- 驾驶舱、Todo 面板、审批与定时唤醒
- Sparkle 自动更新

作者本人每天用它带着一批 session 开发这个仓库本身。

### 代码在，但这份开源仓库里开箱不可用

- **云端登录**（邮箱验证码 / Apple / Google）。
  `Sources/Services/CrewHostedConfig.swift` 里的四个后端坐标是**占位值**，
  这份仓库不携带任何真实后端。点登录时 app 会当场告诉你这件事 —— 判据是
  `CrewHostedConfig.isConfigured`，代码里的这一处是唯一真值，这段文档只是
  转述它。要接你自己的 Supabase + Turnstile，换掉那四个常量即可。

### 不在这一半里

- **跨设备遥控与远端审批**（iPad / iPhone 上看和遥控 Mac 上的 crew）。
  仓库里有 iOS/iPad target 和跨端协议代码，但这条线属于登录态的增值层，
  归另一个尚未开源的生态管，**不属于开源这一半**，也从未分发过任何 iOS 包。
  这不是「做了一半」，是这个项目的边界：**本地必须自洽，登录只是叠加。**
  它今天缺什么，`docs/tech-debt.md` 里记着，没有粉饰。

## 自己构建

**构建依赖**

- macOS 14+ / Xcode 16+
- 不需要装 XcodeGen —— 下面那个脚本会按 `.xcodegen-version` 把对的版本
  取到 `.tools/`（gitignored），不动你系统里已装的任何东西

工程文件由 `project.yml` 生成，改完 `project.yml` 要重跑生成脚本。

```bash
git clone https://github.com/syncmeta/PendingCrew.git
cd PendingCrew
scripts/gen-project.sh --fetch
xcodebuild -project PendingCrew.xcodeproj -scheme PendingCrew \
  -destination 'platform=macOS' build
```

**不需要 Apple 开发者账号。** 仓库默认走 ad-hoc 签名（`Config/Signing.xcconfig`），
没有证书也能编能跑。只有要动云端登录 / 钥匙串那条路径时才需要一个稳定的签名
身份 —— 那时把 `Config/Local.xcconfig.example` 复制成 `Config/Local.xcconfig`（已 gitignore）
填上你自己的 Team ID。

跑测试（单测 bundle 只挂 macOS）：

```bash
xcodebuild -project PendingCrew.xcodeproj -scheme PendingCrew \
  -destination 'platform=macOS' test
```

> `CrewChatOpenCostTests` 用的是不入版本库的真实群聊数据，干净 clone 上会
> 报 skip 并打出取数据的命令 —— 那是预期的，不是失败。

iOS / iPad 端编译验证：

```bash
xcodebuild -project PendingCrew.xcodeproj -scheme PendingCrew \
  -destination 'generic/platform=iOS Simulator' build
```

三端共用一套源码，改动请三端都编一遍 —— 只编 Mac 会让漏 `#if os(macOS)` 的
AppKit 调用把 iOS 端静默打红。

## 目录

```
project.yml             XcodeGen 工程定义（唯一真值，别手改 .xcodeproj）
.xcodegen-version       生成 .xcodeproj 用哪一版 XcodeGen —— 由仓库说了算，不由各机器的 brew
Config/Signing.xcconfig        签名默认值（ad-hoc）；本机覆盖写 Config/Local.xcconfig
Info.plist
Sources/
  Mac/                  macOS 专有：LocalRunner（agent 子进程）、Mac 界面
  Mcp/                  crew-comms MCP server —— agent 通过它读写白板、@ 人、请示
  Stores/               本地持久化：白板、Todo、审批、唤醒、crew 树
  Chat/                 群聊 UI（气泡 / Markdown / composer）
  Remote/               跨端 WS 协议与 viewer 客户端（见「状态」）
  Views/ Services/ Models/ Support/
Resources/              Assets、entitlements、Prompts
Tests/PendingCrewTests/ XCTest（macOS）
Shared/AppUpdate/       Sparkle 自动更新 + 构建版本戳
scripts/                本地小工具 + release/（Developer ID 签名、公证、发 feed）
docs/                   architecture.md（架构导览：想改 X 去哪个文件）
                        release-macos.md（发版）、tech-debt.md（已知的债）
docs/internal/          开发过程记录，写完即冻结，不随代码更新
```

数据都在本机 `~/Library/Application Support/PendingCrew/`，不上传任何地方。

## 参与

见 [CONTRIBUTING.md](CONTRIBUTING.md)。安全问题请走 [SECURITY.md](SECURITY.md)，
不要开公开 issue。

## 许可与免责

[MIT](LICENSE)。随便用、改、分发、拿去商用，保留版权声明即可。

按现状提供，不作任何担保。**PendingCrew 会在你的机器上拉起 coding agent 子进程，
并让它们在你指定的目录里读写文件、执行命令。** 请只把它指向你信得过的工作目录，
并且明白你起的 agent 能做什么 —— 这个 app 不替它们的行为负责。
