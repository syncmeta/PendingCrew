# 安全问题

**不要开公开 issue。** 发邮件到 **security@pendingname.com**，或者用 GitHub 的
[私密漏洞报告](https://github.com/syncmeta/PendingCrew/security/advisories/new)。

这是个人项目，没有安全团队，也没有赏金。我会尽力在几天内回你一句，但请别按
企业 SLA 期待。

## 先看这里 —— 有些「问题」是设计如此

报之前先对一下这几条，能省掉双方的时间：

- **PendingCrew 会在你的机器上执行任意命令。** 它拉起 `claude` / `codex` 子进程，
  这些 agent 会在你指定的工作目录里读写文件、跑命令。**这是这个 app 的全部功能，
  不是漏洞。** 只把它指向你信得过的目录。
- **agent 跑在 runner 自己的自动权限档下。** 常规可逆操作不逐个请示，这是刻意的
  设计（不然它没法自主干活）。你不接受这个前提就别用它。
- **群聊白板是明文 JSON**，在 `~/Library/Application Support/PendingCrew/`。
  文件权限就是你 home 目录的权限，没有额外加密。
- `Sources/Services/CrewHostedConfig.swift` 里的后端坐标是**占位值**，不是泄露的
  凭据。
- `M42BKJN82S`（Apple Team ID）不是秘密 —— 任何签过名的 app 用 `codesign -dv`
  都能读出来。

## 值得报的

- 让 agent 的输出能够**越出它被授予的工作目录**去影响别处
- 白板 / MCP 通道上的注入：外部内容被当成指令执行，跨 crew 或跨 session 越权
- 自动更新链路（Sparkle appcast、签名校验）能被篡改或降级
- 钥匙串 / 凭据处理上的错误
- 任何能让第三方在用户不知情下让本机 agent 干活的路径

报告里请带上：怎么复现、你用的版本（关于 → 版本号，或 `Info.plist` 里的
`CFBundleVersion`）、macOS 版本。

## 支持范围

只支持最新一个 Release。老版本不打补丁 —— 用自动更新升上来。
