<p align="center">
  <img src="docs/app-icon.png" width="128" alt="PendingCrew 应用图标" />
</p>
<h1 align="center">PendingCrew</h1>

<p align="center">
  The Harness of Harness.
</p>
<p align="center">
  <a href="LICENSE"><img alt="License" src="https://img.shields.io/badge/license-MIT-blue" /></a>
  <img alt="Swift" src="https://img.shields.io/badge/lang-Swift%205-F05138?logo=swift&logoColor=white" />
  <img alt="Platform" src="https://img.shields.io/badge/platform-macOS%2014%2B-lightgrey" />
  <a href="https://github.com/syncmeta/PendingCrew/releases"><img alt="Release" src="https://img.shields.io/badge/release-v0.1.13-informational" /></a>
</p>

<p align="center">
  <a href="README.md">中文</a> · <a href="README_EN.md">English</a>
</p>


在 Mac 上和多个 coding agent 深度协作 像企业一样安排和管理它们

比 Agent teams 的扩展性强多了😁 企业用层级来管理大项目是有道理的 我就是想尽量利用这些道理

这个 App 强调**人和 AI 深度协作，提供人机协作的操作台，不是让人当甩手掌柜！！**一个企业要想好，老板的决策至关重要。同理推向 AI。



![PendingCrew 主界面：右侧成员列表里 5 个成员，机长挂 Opus、三个 worker 挂 GPT-5.6-Sol —— 不同厂家的 agent 在同一个群里共事。中间群聊里正在发生真实协作：「终端树渲染」报告提交时撞上共享 Git index 竞态，「HTML 组织图渲染」把这件事挂成待决策、问机长是自己移出去还是由机长统一重整提交；另外两个成员正在输入。](docs/screenshots/crew-collaboration.png)

<p align="center"><sub>真实截图</sub></p>

## 快速开始

安装包： [Releases](https://github.com/syncmeta/PendingCrew/releases)

或用 Homebrew 安装：

```bash
brew install --cask syncmeta/tap/pendingcrew
```

作为与 Claude Code / Codex 协作的平台，你电脑上得先有它们。暂时不能接入其它 Harness。

要做什么事就拉一个群（这里的一个个群，叫机组/Crew）然后在群里说你想做什么。如图：

![image-20260826124145407](/Users/hey/Untitled/Pendingname/PendingCrew/docs/screenshots/crew-1.png)

一个机组可以有父机组，可以有子机组。每个机组默认有机长和各个干活的 Agent 成员。你也可以拉其他联网的 Agent，或者真人，或者 PendingBot 里的机器人和真人进到机组里，一起做事。

机组之间可以形成层级关系，你可以根据具体的目标和任务安排组织层级。可以直接叫机长帮你操作、安排。你给大致方案就行。当然你也可以全权交给它，只是人事调整最好由人类领导来亲自决定。

布置工作时，把输入框左侧的 To Do 图标点亮。建立的 To Do 会记录在册，你可以在 To Do List 中查看每一条 To Do 对应的回应。

时刻注意：你是 Agent 们的领导，人类社会的组织中对领导的要求通常是最高的。我希望 PendingCrew 能让人在用 AI时保持足够的认知参与，同时又降低这个“足够”的门槛，让人在需要判断时有能力判断，把更多的认知资源拿去撬动原本需要更高认知成本才能做的事情。所以我设计群聊、通讯录等等非常直观的交互方式，尽可能让你，让我，把可贵又可怜的认知负荷腾出来。

## 系统要求

- macOS 14 (Sonoma) 或更新
- 本机**已经装好并登录** [Claude Code CLI](https://claude.com/claude-code) 或 [Codex CLI](https://developers.openai.com/codex/cli)  至少一个

## 我精心撰写的文档

 **<https://docs.pendingname.com/pendingcrew>**

## 仓库结构

```
project.yml             XcodeGen 工程定义（唯一真值，别手改 .xcodeproj）
.xcodegen-version       用哪一版 XcodeGen 生成 .xcodeproj —— 由仓库说了算，不由各机器的 brew
Config/Signing.xcconfig 签名默认值（ad-hoc）；本机覆盖写 Config/Local.xcconfig
Info.plist
Sources/
  PendingCrewEntry.swift 进程总入口：启动 GUI，或按参数充当 MCP / hook helper
  PendingCrewApp.swift  SwiftUI App 入口
  Mac/                  本机 runner、长期服务和 macOS 主界面（含少量跨端复用代码）
  Mcp/                  crew-comms MCP server —— agent 通过它读写白板、@ 人、请示
  Stores/               本地持久化：白板、Todo、审批、唤醒、crew 树
  Chat/                 群聊 UI（气泡 / Markdown / composer）
  Views/                跨端 / iOS 界面
  Services/             后端抽象与本地 crew 模型层
  Models/               crew、驾驶舱等值类型
  Support/              跨模块的纯逻辑与小工具
Resources/              Assets、entitlements、Prompts
Tests/
  PendingCrewTests/     XCTest（macOS）
  Fixtures/             群聊性能与终端解析测试语料
Shared/
  AppUpdate/            Sparkle 自动更新 + 构建版本戳
  scripts/              构建版本戳脚本
scripts/                本地小工具 + release/（Developer ID 签名、公证、发 feed）
packaging/homebrew/     Homebrew Cask 模板
docs/                   architecture.md（架构导览）、release-macos.md（发版）、
                        tech-debt.md（已知的债）、screenshots/
docs/internal/          开发过程记录，写完即冻结，不随代码更新
.github/workflows/      CI（三个不需要凭据的门）
```
