# 0.1.35 停在公证这一步（2026-09-12 02:54）

基准提交：`f9b17c0`（已在 `origin/main`）。

## 做完了的

| 步骤 | 状态 |
|---|---|
| 闸门 `release-gate.sh a42a2dd` | ✅ 2687 条 / 0 红，两端 BUILD SUCCEEDED，工程无漂移，文档引用腐烂 0 |
| 版本号 + 更新日志 | ✅ `f9b17c0` |
| `gen-project.sh --fetch` 重生成 | ✅ 无漂移 |
| **push main**（推 tag 之前那一步） | ✅ `811259b..f9b17c0` |
| ARCHIVE / EXPORT | ✅ SUCCEEDED，dSYM 已归档到 `dist/symbols/pendingcrew/0.1.35+20707.67949` |
| 公证 | ❌ **停在这里** |

**tag 没造、GitHub Release 没建、R2 没发、tap 没动** —— 没有半成品要收拾。

## 停在哪

```
Error: No Keychain password item found for profile: pendingcrew-notary
Run 'notarytool store-credentials' to create another credential profile.
```

**这不等于「凭据没建过」**：同一台机器、同一个 profile 名、同一条命令，
两小时前（约 01:00）发 0.1.35 的前一版成功走完全程 ——
`v0.1.34` 现在是公开的非草稿 Release，带 dmg 和 zip 两个附件。
所以凭据是**存在**的，这次是**取不到**。

与当晚另一场故障（数据目录 EPERM，00:47 起未恢复）是不是同一件事，**没有判据**，
没有量过，不写结论。

## 接着做

人在这台机器上确认钥匙串能取到那个 profile（必要时 `xcrun notarytool store-credentials`
重存一次），然后从公证这一步往下接着跑：

```
PENDING_NOTARY_PROFILE=pendingcrew-notary PENDING_PUBLISH_R2=1 sh scripts/release/build-macos-update.sh f9b17c0
sh scripts/release/make-dmg.sh <上一步产出的 zip> dist/releases/pendingcrew
sh scripts/release/publish-github-release.sh 0.1.35
```

脚本会从头重跑一遍构建（幂等），不需要先清什么。
