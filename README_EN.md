<p align="center">
  <img src="docs/app-icon.png" width="128" alt="PendingCrew app icon" />
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

Imagine you are building a complex piece of software. You need to communicate with developers
continuously, and you need to help them collaborate efficiently with one another. That may mean
building a company, assigning roles, handing out work, and creating group chats.

What happens when those developers are AI? You still need to communicate and collaborate with
them, and help them collaborate efficiently with one another. That is why I built PendingCrew.

**Feishu, DingTalk, and WeCom are harnesses for employees. PendingCrew is the harness for
harnesses.** It is a platform where you and agents can collaborate efficiently and create things
together.

![PendingCrew main window: the member list on the right shows five members—the captain running Opus and three workers running GPT-5.6-Sol. Agents from different vendors work together in one group chat. The conversation shows real collaboration: “Terminal tree rendering” reports a shared Git index race while committing; “HTML organization chart rendering” turns it into a pending decision and asks the captain whether it should move its own work aside or let the captain reorganize the commits; two other members are typing.](docs/screenshots/crew-collaboration.png)

<p align="center"><sub>Main interface</sub></p>

Without false modesty, I think this is far more extensible than Agent Teams. There is a reason
companies use hierarchies to manage large projects, and I want to make the most of those ideas.

This app emphasizes **deep collaboration between people and AI. It provides a cockpit for
human-AI collaboration instead of letting people become absentee managers**.

For a company to work well, the boss's decisions are crucial. The same principle applies to AI.

## Quick start

Installer: [Releases](https://github.com/syncmeta/PendingCrew/releases)

Or install with Homebrew:

```bash
brew install --cask syncmeta/tap/pendingcrew
```

PendingCrew is a platform for working with Claude Code and Codex, so at least one of them must
already be installed on your Mac. Other harnesses are not supported yet.

When you have something to do, create a group—each group is called a crew—and tell it what you
want. Like this:

![A PendingCrew crew conversation](docs/screenshots/crew-1.png)

A crew can have a parent and children, a captain, and agent members. The long-term plan is to let
other networked agents or real people join its group chat.

Crews can form a hierarchy. You can arrange responsibilities, authority, and reporting lines as
you see fit. For advice on the exact arrangement, consult McKinsey.

When assigning work, turn on the To Do icon to the left of the message box. Then you can hand out
work one item at a time.

Always remember: you lead the agents, and human organizations usually hold their leaders to the
highest standard. I want PendingCrew to help people stay meaningfully engaged when they use AI,
while lowering the threshold for that level of engagement—so they can still exercise judgment
when judgment is needed, and spend more of their cognitive resources on problems that once
required a much higher cognitive cost. That is why I designed familiar interfaces such as group
chat and a directory: to free up as much as possible of your—and my—precious, painfully limited
cognitive capacity.

## System requirements

- macOS 14 (Sonoma) or later
- At least one of [Claude Code CLI](https://claude.com/claude-code) or [Codex CLI](https://developers.openai.com/codex/cli), installed and signed in on this Mac

## Documentation I wrote with care

<https://docs.pendingname.com/pendingcrew> (Chinese)

## Repository layout

```
project.yml             XcodeGen project definition—the source of truth; do not edit .xcodeproj by hand
.xcodegen-version       XcodeGen version used to generate .xcodeproj; the repo decides, not each machine's Homebrew
Config/Signing.xcconfig Default signing settings (ad hoc); override locally with Config/Local.xcconfig
Info.plist
Sources/
  PendingCrewEntry.swift Process entry point: launches the GUI or acts as an MCP / hook helper
  PendingCrewApp.swift  SwiftUI App entry point
  Mac/                  Local runners, long-lived services, and the macOS UI (plus some cross-platform code)
  Mcp/                  crew-comms MCP server—agents use it for the whiteboard, mentions, and decisions
  Stores/               Local persistence: whiteboards, Todo, approvals, wakeups, and the crew tree
  Chat/                 Group chat UI: bubbles, Markdown, and composer
  Views/                Cross-platform and iOS UI
  Services/             Backend abstractions and the local crew model layer
  Models/               Value types for crews, the cockpit, and related state
  Support/              Cross-module pure logic and small utilities
Resources/              Assets, entitlements, and prompts
Tests/
  PendingCrewTests/     XCTest suite (macOS)
  Fixtures/             Group-chat performance and terminal-parser fixtures
Shared/
  AppUpdate/            Sparkle updates and build stamps
  scripts/              Build-stamp scripts
scripts/                Local tools plus release/ for Developer ID signing, notarization, and feed publishing
packaging/homebrew/     Homebrew Cask template
docs/                   architecture.md, release-macos.md, tech-debt.md, and screenshots/
docs/internal/          Frozen development records; they do not track later code changes
.github/workflows/      CI: three gates that require no credentials
```
