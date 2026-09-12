# 0.1.35 停在公证这一步（2026-09-12 02:54）

<!-- doc-ref-base: 87c4d9d -->
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

---

## 08:17 更正：**停得并不干净**

上面写着「tag / Release / R2 / tap 全都还没动，没有半成品要收拾」。**最后半句是错的。**

`dist/updates/pendingcrew/PendingCrew-0.1.35.zip`（13.7 MB，02:54 落的）一直躺在
**Sparkle feed 目录**里。实测它**没有公证票据**：

```
xcrun stapler validate … → PendingCrew.app does not have a ticket stapled to it.
spctl -a -vv -t exec …   → rejected / source=Unnotarized Developer ID
```

原因在脚本自己身上：那一段本来是「先 `ditto` 进 `$release_dir` → 提交公证 →
staple → 再 `ditto` 覆盖」。公证失败时 `set -e` 当场退出，**第一份、没票据的 zip
就永远留在 feed 目录里**，而 `generate_appcast` 扫的正是那个目录。下一次发别的版本
时它是候选更新，可能被签进 feed 发给所有人 —— 用户那边 Gatekeeper 直接拒，更新链
断掉，而我们这边一切看起来正常。

**两处都改了（同一笔）**：

1. 送公证的那份改落快照临时目录，**只有 staple 之后才往 feed 目录写**。
2. `generate_appcast` 之前加一道闸：feed 目录里**每一个** zip 都必须过
   `stapler validate`，否则拒绝生成并指名是哪一个。堵的是**别人留下的** ——
   feed 目录不入 git，没有任何东西会替我们记得那儿躺了什么。

闸门当场红绿都证了：跑真 feed 目录，0.1.26–0.1.34 那 8 个逐个通过，**停在 0.1.35，
退出码 6**，并打印出它的路径。

**那个 zip 本身没动** —— 删不删是仓库主人的事；重跑发版会用带票据的那份覆盖它。

### 顺手把整条脚本按同一条判据过了一遍

判据是上面那句：**看脚本在失败点之前已经往哪儿写过**（而不是只看 tag/Release/R2/tap）。
`build-macos-update.sh` 里写到快照临时目录**之外**的地方只有这几处：

| 行 | 写什么 | 公证失败时留下什么 |
| --- | --- | --- |
| `mkdir -p "$release_dir"` | 建目录 | 无 |
| `ditto "$xcarchive/dSYMs" "$symbols_dir"` | dSYM 归档 | **一份没发出去的版本的 dSYM**（`dist/symbols/<版本>+<build>/`） |
| 送公证的 zip | 已改落临时目录 | 无（改之前就是这次的病根） |
| 写 feed 的 zip | 只在 staple 之后 | 无 |

**那份孤儿 dSYM 是惰性的**：没有任何东西扫 `dist/symbols/`，它既不会被发出去也不会
污染 feed，只是占地方（每次重试换一个 build 号就多一份）。**所以不动它** ——
留着反而对得上「同 commit 同 build 的符号在哪」这个用途。

生成更新说明那步排在公证**之后**，失败时根本不会跑，不留东西。
