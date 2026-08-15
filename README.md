# PendingCrew

在自己的 Mac 上，把多个 coding agent（Claude Code / Codex）当成一个**小组**来带。

每个 agent 是一个 session，它们共享一块群聊白板：互相看得见对方在说什么，可以
@ 点名、可以互相交接、需要人拍板时会把问题发到群里等你回话。你在一个界面里
看全部人的进度，而不是开七个终端窗口来回切。

- **Mac** —— 完整形态：起 session、群聊、终端、驾驶舱。
- **iPad / iPhone** —— 同一份 SwiftUI 工程，用来在别处看和遥控 Mac 上的 crew。

## 依赖

**运行 PendingCrew 之前，本机必须已经装好并登录至少一个 coding agent CLI：**

- [Claude Code](https://claude.com/claude-code) —— 命令 `claude` 在 `PATH` 上，且已登录
- 或 [Codex CLI](https://developers.openai.com/codex/cli) —— 命令 `codex` 在 `PATH` 上，且已登录

PendingCrew 自己**不持有任何模型 API key、也不代付任何费用**：它是把这两个 CLI
作为子进程拉起来，烧的是你自己的订阅额度。两个都没装的话，app 打得开，但起不了
session。

构建依赖：

- macOS 14+ / Xcode 16+
- [XcodeGen](https://github.com/yonaskolb/XcodeGen)（`brew install xcodegen`）

## 构建

工程文件由 `project.yml` 生成，改完 `project.yml` 要重跑 `xcodegen`。

```bash
xcodegen
xcodebuild -project PendingCrew.xcodeproj -scheme PendingCrew \
  -destination 'platform=macOS' build
```

跑测试（单测 bundle 只挂 macOS）：

```bash
xcodebuild -project PendingCrew.xcodeproj -scheme PendingCrew \
  -destination 'platform=macOS' test
```

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
Info.plist
Sources/
  Mac/                  macOS 专有：LocalRunner（agent 子进程）、Mac 界面
  Mcp/                  crew-comms MCP server —— agent 通过它读写白板、@ 人、请示
  Stores/               本地持久化：白板、Todo、审批、唤醒、crew 树
  Chat/                 群聊 UI（气泡 / Markdown / composer）
  Views/ Services/ Models/ Support/ Remote/
Resources/              Assets、entitlements、Prompts
Tests/PendingCrewTests/ XCTest（macOS）
Shared/AppUpdate/       Sparkle 自动更新 + 构建版本戳
scripts/                本地小工具 + release/（Developer ID 签名、公证、发 feed）
```

## 配置

`Sources/Services/CrewHostedConfig.swift` 里的 Supabase / Turnstile 常量是**占位值**。
只跑本机 crew 的话用不到它们；要接自己的云端后端再填。

签名用的 `DEVELOPMENT_TEAM` 写在 `project.yml` 里，换成你自己的 Apple Team ID。

## 状态

个人作品，开发中。接口和数据格式都还会变。
