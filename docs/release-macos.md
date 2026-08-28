# macOS 发版 —— 怎么出一个能装、能自动更新的包

**一句话**：只跑 `scripts/release/build-macos-update.sh`，别手搓。

```sh
PENDING_NOTARY_PROFILE=pendingcrew-notary scripts/release/build-macos-update.sh [release-ref]
# 要顺带发到 R2（线上自动更新 feed）才加 PENDING_PUBLISH_R2=1
```

`release-ref` 省略时默认 `main`；也可以显式传 commit、tag 或分支。脚本在开工时只解析
一次，打印 `release ref … -> snapshot HEAD <完整 SHA>` 回执，并让干净快照、产物里的
commit 版本戳和 `v<版本>` tag 全部指向这一个 SHA。已有同名 tag 若指向别处会直接拒绝，
不会静默沿用。**工作区脏不脏都不影响所见即所装。**
它包办：构建 Release → Developer ID 签名（含 Sparkle 内嵌件）→ 公证 → staple →
生成更新说明 → `generate_appcast` 签 feed → 打 tag →（可选）发 R2，中间有六道
断言，每一道都对应一次真实踩过的坑（见脚本里的注释）。

## 更新日志 —— `CHANGELOG.md` 是唯一出处

GitHub Release 的正文和 app 内 Sparkle 更新弹窗里那页说明，**都从
`CHANGELOG.md` 里 `## <版本号>` 那一段取**，由 `scripts/release/changelog-section.sh`
统一取段，两边共用同一个实现（各写一份 awk 必然漂移）。

发新版之前，先在 `CHANGELOG.md` 顶上补一段：

```markdown
## 0.1.14

- 一条一句话，说清这版对用的人有什么不同
```

**没写就发不出去**，而且是提前停的：`build-macos-update.sh` 在读到版本号之后
立刻预检这一段，缺了当场退出，不会等公证跑完才发现。`publish-github-release.sh`
同样在传任何产物之前取段，取不到就非零退出。

**故意不做兜底。** 以前这两处一个用 `gh --generate-notes`、一个把
`git log --format=%s` 直接倒进 `<li>`，结果是用户在更新弹窗里读到
「docs: README 按作者手改的版本重写」这种提交标题。回落到自动生成
等于没改，所以取不到就报错，不猜。

手写的例外还留着：`dist/updates/pendingcrew/PendingCrew-<版本>.html` 已经存在时
`generate-release-notes.sh` 原样保留，不覆盖。

## 挂 GitHub Release —— 和「发版」是两件事

**挂 Release 用手边那份已经公证好的产物，不为了发 Release 而构建新版本。**

这条写下来是因为它很容易被绑成一件事，而绑起来的代价是真的：`main` 上随时
可能躺着一批还没人工验收过的改动，为了挂个 Release 把它们构建成新版本，等于
把未验收的东西**推给自更新用户**（Sparkle 那边看的是 build 号）。Release 页面
要的只是「一个能下载能装的包」，那份包已经在 `dist/updates/pendingcrew/` 里了。

所以正常的顺序是：

```sh
# 1. 造 dmg（输入是已公证的 .zip 或 .app；dmg 自己也会签名+公证+staple）
#    输出落在 dist/releases/pendingcrew/ —— 不是 zip 旁边，理由见下面「产物放哪」
PENDING_NOTARY_PROFILE=pendingcrew-notary \
  scripts/release/make-dmg.sh dist/updates/pendingcrew/PendingCrew-<版本>.zip

# 2. 把 tag 推上去（发版脚本已经在本地打过 tag 了）
git push origin v<版本>

# 3. 建 draft，两个产物都挂上（正文自动取自 CHANGELOG.md，并顺手更新 Homebrew tap）
scripts/release/publish-github-release.sh <版本> --draft
```

## 产物放哪 —— dmg 不能待在 Sparkle 的 feed 目录里

```
dist/updates/pendingcrew/     ← Sparkle feed：只放 .zip + appcast.xml + .html，会整目录同步到 R2
dist/releases/pendingcrew/    ← 只给 GitHub Release 用的 .dmg
```

**这不是洁癖，是一道真的闸。** `generate_appcast` 扫描 feed 目录时，看见同一个
bundle version 出现在两个归档里（`X.zip` 和 `X.dmg`）会直接拒：

```
Duplicate updates are not supported. Found archives 'PendingCrew-0.1.13.zip'
and 'PendingCrew-0.1.13.dmg' which contain the same bundle version.
```

2026-08-24 出 0.1.14 时就断在这儿 —— 公证都过了，卡在最后一步。0.1.13 那次没炸
只是因为 dmg 是在 appcast 生成**之后**才造的，顺序碰巧躲过去了。

**`--draft` 不是可选的**：Release 一旦发布就会给自动更新之外的人一个下载入口，
发布这一下是人的决定，不是脚本的。确认好了再 `gh release edit v<版本> --draft=false`。

两个产物都要挂，各有各的用处：

- **`.dmg` 是给人手动下载的主入口。** `.zip` 解压之后用户多半直接在「下载」
  文件夹里双击，会踩 App Translocation（系统把 app 挪到随机只读路径跑），
  自动更新和本地数据目录都会出怪事。dmg 里那个「应用程序」快捷方式是唯一能把
  人引导到正确安装位置的标准姿势。
- **`.zip` 是 Sparkle 自动更新吃的那一份**（Sparkle 只认 zip），手动装的人用不到。

挂 Release **不需要**也**不应该**开 `PENDING_PUBLISH_R2` —— 那是往线上自动更新
feed 发东西，跟 GitHub Release 是两条独立的分发路径。

验一个 dmg 是不是真能装（`make-dmg.sh` 结尾会自己跑，但换机器/手工造包时照做）：

```sh
spctl -a -t open --context context:primary-signature -vvv <dmg>   # 判 dmg 用这条
xcrun stapler validate <dmg>
```

注意判 dmg 的口径（`-t open --context context:primary-signature`）和判 app 的
（`-t install`）**不是一回事**，别拿后者去验 dmg。

## 公证凭据（`PENDING_NOTARY_PROFILE`）

profile 名：**`pendingcrew-notary`**，存在**登录钥匙串**里。

2026-08-19 之前这台机器上**根本没有**这个 profile —— 于是发版脚本一次都没跑成过，
装机的包全是手搓的，**没过公证**（`spctl` 判 `Unnotarized Developer ID`：自用没事，
换台机器就会被拦成「无法验证开发者」）。别再走那条路。

profile 用的是本机那把 **App Store Connect API key**（团队 `M42BKJN82S`，与
PendingBot 传 TestFlight 是同一把；公证是这把 key 的正当用途）。重建方法：

```sh
xcrun notarytool store-credentials "pendingcrew-notary" \
  --key   ~/.appstoreconnect/private_keys/AuthKey_<KEYID>.p8 \
  --key-id <KEYID> --issuer <ISSUER-UUID>
```

`<KEYID>` / `<ISSUER-UUID>` / `.p8` 的具体值**不写进仓库**。它们在本机
`~/.appstoreconnect/` 下；换机器时问人要，别贴进任何聊天或提交。

验证 profile 活着：`xcrun notarytool history --keychain-profile pendingcrew-notary`。

**公证失败或卡住就停下来说**，不要退回「关掉公证先装上」—— 那正是上面那个洞。

## build 号

`天.秒`（纪元日 + 当天秒数，零填充），从时钟派生、不从 git 历史算，理由见脚本注释。
脚本会拉线上 appcast 比一次，**不大于线上最大值就拒绝构建**（fail-closed：拉不到
feed 也拒绝，网络失败不许和「首发」共用放行路径）。

## 装到本机

发版脚本只产出包，**不负责装**。装的那一步要等人把正在跑的 PendingCrew 退掉
（所有 session 会随旧进程结束），所以走一个挂在 launchd 底下的安装器：它与
PendingCrew 不同进程组，用户按 ⌘Q 不会连带杀掉它；等待窗口 90 分钟，**超时原样
放弃、一个文件都不动**。顺序是：冷备份数据目录 → 旧 app 移到回滚位 → 装新的 →
自动重开。

- 回滚位：`~/Library/Application Support/PendingCrew-app-rollback/PendingCrew-old-<时间>.app`
  （**不放桌面** —— `~/Desktop` 受 TCC 保护，非交互进程读写会被拒）
- 数据备份：`~/Library/Application Support/PendingCrew-databackup-<时间>`
- 回滚命令：`rm -rf /Applications/PendingCrew.app && cp -R <回滚位的那份> /Applications/PendingCrew.app`

## 发版闸门记录 —— 每一版说清「测过没有」

发版全量（钉死 worktree 那一趟）的结果**逐版记在这里**，包括「没跑」。

理由只有一条：**下一个人会拿上一版的状态当基线。** 一版真跑过、绿的，跟一版
根本没跑过，在「可以照着发」这件事上完全不同；而这两者在白板上都长得像
「发出去了」。白板会被刷走，这张表不会。

**写「没跑」比写「大概没问题」有用。** 同理，有飘红就照实写飘红 —— 把红说成
绿，下一个人拿到的是一个假基线，比没有基线更坏。

| 版本 | 闸门 | 结果 |
| --- | --- | --- |
| 0.1.19 | 跑了，钉在 `bef3095` | **1722 例 / 3 skip / 0 失败**，`** TEST SUCCEEDED **`，macOS 与 iOS Simulator build 都过。3 条 skip 逐项是 `AgentTuiFixtureRecorder.testRecord` 和两条现场目录基准，与登记构成一致；`CrewChatOpenCostTests` 8 条全部真执行且 passed；具名失败为空；HEAD / TREE 前后逐字相同。**真全绿。** |
| 0.1.18 | 跑了，钉在 `a565032` | **1717 例 / 3 skip / 0 失败**，`** TEST SUCCEEDED **`，两端 build 都过。四条判读逐条核过：3 条 skip 就是登记在案的那三条（构成对上，不是只对数字）；`CrewChatOpenCostTests` 8 条全 Executed 且 passed —— 其中 `test_打开LED驱动板一次重排在预算内`、`test_整表一次布局_窗口化前后` 正是 0.1.16 飘红的那两条，这次真跑真过；具名失败为空；HEAD/TREE 前后逐字相同。**真全绿。** |
| 0.1.16 | 跑了，钉在 `b8bd679` | 1692 例 / 3 skip / **4 条飘红**（性能预算断言 · NSCache 回收）。取证确认与本次改动无关：单独跑全绿、同批用例在别的提交上红绿交替、被测类型这一版零提交。**不是全绿。** |
| 0.1.17 | **没跑** | 白板上没有这一版的任何测试记录，事后也没有补跑。**不得当作基线。** 包本身的签名 / 公证 / tag 对账都过（tag `v0.1.17` = 产物 Info.plist 里的 `BuildStampCommit` = `fedb697`），但那验的是「包是不是它自称的那个」，不是「代码对不对」。 |

从 2026-08-27 起 CI 在**每次 push main** 上跑三端编译 + 单测（见
`.github/workflows/ci.yml`）。这让 main 有了一条持续的证据线，但**不取代发版
闸门**，有两条边界：

1. **量的不是同一个 commit。** 闸门跑的是钉死 worktree 的那一趟，量「这个包的
   来源提交」；CI 量「main 当时那一点」。main 活跃时这两点常常不同。
2. **CI 的绿盖不住性能预算那一族。** `CrewChatOpenCostTests` 整个文件靠真实
   白板 fixture 驱动，fixture 不进仓库，所以在 runner 上**全部 XCTSkip**
   （首趟实测：本地闸门 skip 3、CI skip 14，差的 11 条就是它）。而那一族正是
   最常飘红的那一族 —— **CI 永远不会因为它红，也就永远不会替你抓到它的真回归。**
   要量它只能走本地闸门（`cp -R` 把 fixture 拷进钉死的 worktree）。

首趟全量（`4909b72`）：**1694 例 / 14 skip / 0 失败**，`** TEST SUCCEEDED **`，
iOS 端 `** BUILD SUCCEEDED **`。这是 0.1.17 那批代码第一次真正被 CI 编译和测试。
