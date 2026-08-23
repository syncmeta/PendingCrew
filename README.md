<p align="center">
  <img src="docs/app-icon.png" width="128" alt="PendingCrew 应用图标" />
</p>
<h1 align="center">PendingCrew</h1>

<p align="center">
  The Harness of Harness.
  <br />
  <em>在自己的 Mac 上，把多个 coding agent 当成一个小组来带</em>
</p>

<p align="center">
  <a href="LICENSE"><img alt="License" src="https://img.shields.io/badge/license-MIT-blue" /></a>
  <img alt="Swift" src="https://img.shields.io/badge/lang-Swift%205-F05138?logo=swift&logoColor=white" />
  <img alt="Platform" src="https://img.shields.io/badge/platform-macOS%2014%2B-lightgrey" />
  <a href="https://github.com/syncmeta/PendingCrew/releases"><img alt="Release" src="https://img.shields.io/badge/release-v0.1.13-informational" /></a>
</p>

Claude Code、Codex 这些 coding agent 各自带着自己的 harness 跑。PendingCrew 是**罩在它们外面的那一层**：
在你自己的 Mac 上，把多个 agent 当成一个**小组**来带。

![PendingCrew 主界面：右侧成员列表里 5 个成员，机长挂 Opus、三个 worker 挂 GPT-5.6-Sol —— 不同厂家的 agent 在同一个群里共事。中间群聊里正在发生真实协作：「终端树渲染」报告提交时撞上共享 Git index 竞态，「HTML 组织图渲染」把这件事挂成待决策、问机长是自己移出去还是由机长统一重整提交；另外两个成员正在输入。](docs/screenshots/crew-collaboration.png)

<p align="center"><sub>五个成员、两家模型、一块共享白板。图里那条待决策是真撞出来的——三个 agent 共用一个工作目录，提交时撞上了 Git index 竞态，于是有人把问题挂到群里等机长拍板。</sub></p>

## 快速开始

**1. 先确认你有前提。** 这条不满足的话，app 打得开，但一个 session 都起不来：

- **macOS 14 (Sonoma) 或更新**，Apple Silicon 或 Intel 都行
- 本机**已经装好并且登录过** [Claude Code](https://claude.com/claude-code)
  或 [Codex CLI](https://developers.openai.com/codex/cli) —— **至少一个**，
  命令要在 `PATH` 上。装了但没登录过同样不行。

  ```bash
  claude --version   # 或者
  codex --version
  ```

  PendingCrew **不持有任何模型 API key，也不代付任何费用**。它只是把这两个 CLI
  作为子进程拉起来，烧的是你自己的订阅额度。

**2. 下载安装。** 到 [Releases](https://github.com/syncmeta/PendingCrew/releases)
取 `.dmg`，打开，把 PendingCrew **拖进「应用程序」**再启动。包过了 Apple 公证，
双击不会出现「无法验证开发者」。

> 别直接在「下载」文件夹里双击 —— macOS 会把从下载目录运行的 app 挪进一个随机
> 只读路径（App Translocation），自动更新和本地数据目录都会出怪事。

**3. 建一个 crew，跟机长说话。** 新建 crew 时指定一个工作目录，第一个 session
（机长）会自己起来。你把想做的事讲给它，它负责拆活、决定起谁去干、替卡住的成员拍板。

不需要登录、不需要账号、不需要任何后端。

## 这是什么

每个 agent 是一个 session，它们共享一块**群聊白板**：互相看得见对方在说什么，可以
@ 点名、可以互相交接、需要人拍板时会把问题发到群里等你回话。你在一个界面里看全部人
的进度，而不是开七个终端窗口来回切。

**机长（captain）** 是 crew 里的第一个 session。你只跟机长说话，它带着其余人跑：
读你的意图、把活拆开、起 worker、必要时拆出子 crew、替被卡住的成员做决定。

机制上没有魔法，闭环的介质就是文件系统：

```
   人 ──▶ PendingCrew.app ───拉起 PTY / stdio───▶ claude / codex 子进程
              │  ▲                                        │
              │  │ 目录监听                                │ MCP 工具调用
              ▼  │                                        ▼
        ~/Library/Application Support/PendingCrew/whiteboards/*.json
                          （群聊白板 = 一堆 JSON 文件）
```

agent 通过一个 **MCP server** 读写白板，而那个 MCP server 就是 **app 自己的二进制
换个 argv 再跑一遍**。数据全部落在本机 `~/Library/Application Support/PendingCrew/`，
不上传任何地方。

## 文档

完整文档在 **<https://docs.pendingname.com>**。

仓库内另有两份给动手的人看的：[`docs/architecture.md`](docs/architecture.md)
（架构导览，回答「我想改 X 该去哪个文件」）和
[`docs/tech-debt.md`](docs/tech-debt.md)（已知的债，没有粉饰）。

## 功能

### 现在能做的 —— 都是天天在用的

不需要登录、不需要后端，装上就是全部：

- 建 crew、起 Claude Code / Codex session，**共享目录或各自独立的 git worktree**
- 群聊白板 —— session 之间互相看见、@ 点名、交接、把问题抛给你拍板
- **每个 session 背后是一个真 PTY 终端**，你随时可以接管键盘
- 机长自主派活、起子 session、拆子 crew
- 驾驶舱、Todo 面板、审批与定时唤醒
- Sparkle 自动更新（发版流水线含 Developer ID 签名 + Apple 公证 + staple）

作者本人每天用它带着一批 session 开发这个仓库本身。

![同一个界面的右栏展开成机长的终端：顶上写着「机长 / Claude Code / Opus」，正在跑 `Bash(python3 …)`，输出折叠成「+128 lines (ctrl+o to expand)」并标注 Allowed by auto mode，底部是 `auto mode on (shift+tab)` 和 `Worked for 56s`。](docs/screenshots/session-terminal.png)

<p align="center"><sub>点开任一成员，右栏就是它那个真正的终端——不是日志视图，是活的 PTY：你可以直接接管键盘，也能看见它此刻在跑哪条命令、跑了多久。</sub></p>

### 代码在，但这份仓库里开箱不可用

- **云端登录**（邮箱验证码 / Apple / Google）。
  `Sources/Services/CrewHostedConfig.swift` 里的四个后端坐标是**占位值**，这份仓库
  不携带任何真实后端。点登录时 app 会当场告诉你这件事 —— 判据是
  `CrewHostedConfig.isConfigured`，代码里那一处才是唯一真值，这段文档只是转述它。
  要接你自己的 Supabase + Turnstile，换掉那四个常量即可。

### 不在这一半里

- **跨设备遥控与远端审批**（在 iPad / iPhone 上看和遥控 Mac 上的 crew）。
  仓库里有 iOS/iPad target 和跨端协议代码，但**这条线从未接通过**，也从未分发过
  任何 iOS 包。它属于登录态的增值层，归另一个尚未开源的生态管。
  这不是「做了一半」，是这个项目的边界：**本地必须自洽，登录只是叠加。**
  它今天缺什么，`docs/tech-debt.md` 里记着。

### 说清楚期望

**这就是个个人实验，别把它当商业产品。** 开发中，接口和数据格式都还会变。
上面这三档是照实写的，请按它判断这东西现在能给你什么。

## 开源

- **许可**：[MIT](LICENSE)。随便用、改、分发、拿去商用，保留版权声明即可。
  23 个 SPM 依赖（7 个直接 + 16 个传递）逐个核过 LICENSE 正文，**没有 GPL / LGPL /
  AGPL**，构成是 MIT / Apache-2.0 / BSD。
- **参与**：见 [CONTRIBUTING.md](CONTRIBUTING.md)。三条硬规矩：`project.yml` 是工程
  定义的唯一真值（改完要重新生成并提交 `.pbxproj`，**新增 Swift 文件也算**）、三端
  都要编一遍、绕过约束的临时方案要在 `docs/tech-debt.md` 留痕。
- **安全**：见 [SECURITY.md](SECURITY.md)。**不要开公开 issue** —— 走 GitHub 的
  [私密漏洞报告](https://github.com/syncmeta/PendingCrew/security/advisories/new)。
  先说清楚：这是个人实验项目，没有经过任何安全审计。
- **CI**：每个 PR 都会跑三个门（`.pbxproj` 与 `project.yml` 是否同步 / macOS 编译 +
  单测 / iOS 端编译）。签名证书与公证凭据**一律不进 GitHub secrets**，出包走本地。

**免责**：按现状提供，不作任何担保。**PendingCrew 会在你的机器上拉起 coding agent
子进程，并让它们在你指定的目录里读写文件、执行命令。** 请只把它指向你信得过的工作
目录，并且明白你起的 agent 能做什么 —— 这个 app 不替它们的行为负责。

## 项目架构

- **三端一套源码。** macOS 是完整形态；iOS / iPad 是同一个 SwiftUI 工程编出来的
  另一个产物。只编 Mac 会让漏了 `#if os(macOS)` 的 AppKit 调用把 iOS 端静默打红，
  所以改动要三端都编一遍。
- **app 二进制自己当 MCP server。** 带 `--mcp-serve` / `--mcp-hook` 再跑一遍自己，
  agent 就通过它读写白板、@ 人、请示 —— 不需要额外进程或额外分发物。
- **白板是磁盘上的 JSON**，靠目录监听（`DispatchSource`）合流通知界面，多进程写入
  走文件锁。没有数据库、没有服务端。
- **每个 session 一个真 PTY**（SwiftTerm），不是伪终端日志。
- **测试**：XCTest，只挂 macOS（被测代码基本都在 `#if os(macOS)` 后面）。
  当前 **1,443 个测试，0 失败**（2026-08-22 实测）。干净 clone 上会多出十来条
  skip —— 那是需要不入版本库的真实群聊数据的那一套，属预期，不是失败。
- **构建与发版**：工程文件由 XcodeGen 从 `project.yml` 生成，生成器版本由仓库的
  `.xcodegen-version` 钉死；发版脚本一条龙做 Release 构建 → Developer ID 签名 →
  **Apple 公证 → staple** → 签 appcast → 打 tag。

细节见 [`docs/architecture.md`](docs/architecture.md)。

### 自己构建

不需要 Apple 开发者账号 —— 仓库默认走 ad-hoc 签名（`Config/Signing.xcconfig`），
没有证书也能编能跑。也不需要预先装 XcodeGen，下面的脚本会按 `.xcodegen-version`
把对的版本取到 `.tools/`（gitignored），不动你系统里已装的任何东西。

```bash
git clone https://github.com/syncmeta/PendingCrew.git
cd PendingCrew
scripts/gen-project.sh --fetch
xcodebuild -project PendingCrew.xcodeproj -scheme PendingCrew \
  -destination 'platform=macOS' build      # 编
xcodebuild -project PendingCrew.xcodeproj -scheme PendingCrew \
  -destination 'platform=macOS' test       # 测
xcodebuild -project PendingCrew.xcodeproj -scheme PendingCrew \
  -destination 'generic/platform=iOS Simulator' build   # iOS 端编译验证
```

> 干净 clone 上 `CrewChatOpenCostTests` 会 skip 并打出取数据的命令 —— 那是预期的，
> 不是失败。要动云端登录 / 钥匙串那条路径才需要稳定签名身份：把
> `Config/Local.xcconfig.example` 复制成 `Config/Local.xcconfig`（已 gitignore）
> 填你自己的 Team ID。

## 仓库结构

```
project.yml             XcodeGen 工程定义（唯一真值，别手改 .xcodeproj）
.xcodegen-version       用哪一版 XcodeGen 生成 .xcodeproj —— 由仓库说了算，不由各机器的 brew
Config/Signing.xcconfig 签名默认值（ad-hoc）；本机覆盖写 Config/Local.xcconfig
Info.plist
Sources/
  Mac/                  macOS 专有：LocalRunner（agent 子进程）、Mac 界面
  Mcp/                  crew-comms MCP server —— agent 通过它读写白板、@ 人、请示
  Stores/               本地持久化：白板、Todo、审批、唤醒、crew 树
  Chat/                 群聊 UI（气泡 / Markdown / composer）
  Remote/               跨端 WS 协议与 viewer 客户端（见「不在这一半里」）
  Views/ Services/ Models/ Support/
Resources/              Assets、entitlements、Prompts
Tests/PendingCrewTests/ XCTest（macOS）
Shared/AppUpdate/       Sparkle 自动更新 + 构建版本戳
scripts/                本地小工具 + release/（Developer ID 签名、公证、发 feed）
docs/                   architecture.md（架构导览）、release-macos.md（发版）、
                        tech-debt.md（已知的债）、screenshots/
docs/internal/          开发过程记录，写完即冻结，不随代码更新
.github/workflows/      CI（三个不需要凭据的门）
```

---

<sub>本 README 的结构与项目定位由作者拟定，技术章节由 Claude 补写。其中 1,443 个测试
0 失败来自 2026-08-22 在本机实跑 `xcodebuild … test` 的输出，依赖许可证构成来自逐个
读 23 个 SPM checkout 的 LICENSE 正文，版本号取自 GitHub Releases，
不是从旧文档转抄。</sub>
