# iOS 发版 —— 怎么出一个能进 TestFlight 的包

**一句话**：只跑 `scripts/release/build-ios-testflight.sh`，别手搓。

```sh
# 先空跑一遍：走完 archive + export + 全部断言，在上传那一步停下
scripts/release/build-ios-testflight.sh --dry-run

# 确认没问题了再真发
scripts/release/build-ios-testflight.sh
```

脚本自己钉 `main` HEAD 建干净快照 —— **工作区脏不脏都不影响所见即所装**。
它包办：算 build 号 → 跟 App Store Connect 核不倒退 → `xcodegen` →
archive（iOS，自动签名 + ASC API key）→ `exportArchive` 出 `.ipa` → 八道断言 →
`altool` 上传，中间每一道断言都对应一种「包能出、能上传，然后在别处安静地坏掉」
的失败（见脚本里的注释）。

和 [macOS 那条](release-macos.md) 是**两条独立的分发路径**，别混：

|            | macOS                                   | iOS                              |
| ---------- | --------------------------------------- | -------------------------------- |
| 谁分发      | 我们自己（Sparkle 自动更新 / GitHub Release） | Apple（TestFlight / App Store）    |
| 签名身份    | Developer ID Application                | Apple Distribution（云端管理，本机不用装）|
| 出口        | 公证 + staple 的 `.zip` / `.dmg`          | `.ipa`                           |
| 防倒退闸    | 比线上 appcast 里的最大 `sparkle:version`   | 比 ASC 上已收到的最高 `CFBundleVersion` |
| 脚本        | `build-macos-update.sh`                 | `build-ios-testflight.sh`        |

**build 号编码两条共用**（纪元日「天.秒」），见下面。

## 前置：谁需要什么

1. **Apple 开发者账号，在团队 `M42BKJN82S` 里，角色至少 App Manager。**
   角色不够的表现不是「没权限」四个字，而是 archive 那步创建 profile 时报一句
   看不懂的 provisioning 错误。

2. **App Store Connect API key**，三个值写在 `~/.appstoreconnect/pendingbot.env`
   （`chmod 600`，一行一个 `KEY=value`）：

   ```
   ASC_KEY_ID=<KEYID>
   ASC_ISSUER_ID=<ISSUER-UUID>
   ASC_KEY_PATH=/Users/<you>/.appstoreconnect/private_keys/AuthKey_<KEYID>.p8
   ```

   三个也可以直接设成同名环境变量（优先级更高）。文件名带 `pendingbot` 是历史
   原因：**同一把团队 key** 既给 PendingBot 传 TestFlight、也给 PendingCrew 公证
   macOS 包，不是两把。

   **`.p8` 的文件名必须是 `AuthKey_<KEYID>.p8`** —— `altool` 的 `--api-key` 不吃
   路径，它按这个名字去 `~/.appstoreconnect/private_keys`（以及 `./private_keys`、
   `~/private_keys`、`~/.private_keys`、`$API_PRIVATE_KEYS_DIR`）里找。脚本会在
   开工前就断言这一条，免得等十几分钟构建跑完才收到一句「找不到 key」。

   `<KEYID>` / `<ISSUER-UUID>` / `.p8` 的**值**不写进仓库、不贴进聊天、不进任何
   日志。换机器时问人要。脚本从头到尾只用**路径 + ID**。

3. **Xcode 26.5+**（`xcodebuild`、`altool` 都从这儿来）。

4. **`/usr/bin/python3` 上有 PyJWT**（防倒退闸要给 ASC API 签 JWT）：

   ```sh
   /usr/bin/python3 -m pip install --user pyjwt
   ```

5. **网络**。三处都要：查 ASC 最高 build 号、archive 时向开发者网站申请
   profile/证书、上传。

**不需要**本机装 Apple Distribution 证书。这台机器的钥匙串里只有 Apple
Development 和 Developer ID —— 分发证书由 `-allowProvisioningUpdates` 配合
ASC key 在云端管理（Xcode 自己建、自己用）。

## 每一步在干什么

| # | 步骤 | 干什么 / 为什么 |
| - | ---- | -------------- |
| 0 | 读凭据 | 从环境变量或 `pendingbot.env` 取三个 ID/路径，断言 `.p8` 在、文件名对得上 |
| 1 | 干净快照 | `git worktree add --detach` 钉 `main` HEAD。这台机器工作区常年是脏的（多 session 并行），发出去的包必须来自 `main` |
| 2 | 算 build 号 | 纪元日「天.秒」，从时钟派生（见下） |
| 3 | 防倒退闸 | `asc-highest-build.py` 查 ASC 上最高 build 号，不严格递增就拒绝。**放在 archive 之前**，别白跑十几分钟 |
| 4 | `xcodegen` | 走 `scripts/gen-project.sh --fetch`，生成器版本由仓库的 `.xcodegen-version` 说了算，不由各机器的 brew |
| 5 | 补签名参数 | 往**快照里**现写一份 `Config/Local.xcconfig`（开发机上那份是 gitignored 的，快照里没有） |
| 6 | archive | `-destination 'generic/platform=iOS'` + ASC key 三件套 + `-allowProvisioningUpdates` |
| 7 | exportArchive | `method=app-store-connect` / `signingStyle=automatic` / `destination=export` → 出 `.ipa` |
| 8 | 八道断言 | 见下 |
| 9 | 上传 | `xcrun altool --upload-package`，显式带数字 app id |

产物落在 `dist/testflight/PendingCrew-<版本>-<build>.ipa`（`dist/` 是 gitignored）。

### 签名参数从哪来（以及为什么不写在命令行上）

仓库里被跟踪的 `Config/Signing.xcconfig` 是 **ad-hoc、不带 entitlements** ——
这是有意的，外部贡献者 clone 下来不改任何被 git 跟踪的文件就能编。开发机上那份
身份在 `Config/Local.xcconfig`，而它 **gitignored**，第 1 步那个干净快照里**根本
没有这个文件**。所以发版脚本自己往快照里写一份，不指望开发机上碰巧有。
**别为了发版去改 `Signing.xcconfig` 的默认值。**

那为什么不像 macOS 那条一样直接写在 `xcodebuild` 命令行上（优先级最高、最直接）？
**因为命令行上的构建设置是全局的，连每个 SPM 依赖的 target 都吃。**
`CODE_SIGN_ENTITLEMENTS` 是相对 SRCROOT 的路径，于是 GoogleSignIn / SwiftTerm /
swift-crypto…… 每个包都拿 `Resources/PendingCrew.entitlements` 去自己的 checkout
目录里找，齐刷刷报 `could not be opened`（2026-08-20 实测，十几个包一个不落）。
project 级 config file 只作用于**本工程的** target，SPM 包完全不受影响。

macOS 那条没炸只是因为它的签名身份停在 ad-hoc（`-`），Xcode 根本没走到给这些包
target 签名那一步 —— **不是 iOS 特有的坑**，换成真身份一样会炸。

iOS 这边不能停在 ad-hoc：带 entitlements 的 app 用 ad-hoc 签，Xcode 直接拒
（`has entitlements that require signing with a development certificate`）。
所以 archive 用**开发身份**签，发布身份由 `exportArchive` 按
`method: app-store-connect` 重新签 —— 这是 Apple 官方的分发路径，签名全权交给
Xcode，脚本里不出现任何一行手写的 `codesign`。

### 为什么 export 完再单独上传，而不是一步直传

`exportArchive` 的 `destination` 填 `upload` 就能一步直传 ASC，但那样**中间不落
`.ipa`**，产物长什么样在上传前就没法验。我们要的顺序是「先出包 → 断言全过 →
再传」，所以出包和上传是两步。

### 八道断言

| # | 拦的是什么 |
| - | --------- |
| ① | `.ipa` 到底有没有产出 |
| ② | 版本号/build 号真的落进产物了（占位符 `"1"` 会被 ASC 拒收，但那要等上传往返才知道） |
| ③ | `CFBundleSupportedPlatforms` 是 `iPhoneOS` —— **单 target 两平台**，`-destination` 写错会打出 macOS 产物，`.ipa` 外壳看不出来 |
| ④ | 签的是 `M42BKJN82S` 不是 ad-hoc —— 签名参数漏传时构建照样成功、产物照样是 `.ipa` |
| ⑤ | `embedded.mobileprovision` 在 —— 受限 entitlement 没 profile 授权，装上去点不开，而签名验证全过 |
| ⑥ | entitlements 里没有未展开的 `$(...)`，且 keychain 组带上了 team 前缀（macOS 那边踩过：变量没展开 → 登录态静默丢失，全链路零报错） |
| ⑦ | 源 entitlements 声明的每个键，产物里都还在 —— Xcode 会把 profile 未授权的键**静默剥掉** |
| ⑧ | `get-task-allow` 不是 true（development 签名的标志，带着它上传会被 ITMS 拒），且 `beta-reports-active` 是 true（只有 App Store 分发 profile 才给，是「真按 app-store-connect 重签过」的正面证据）。**这两条是 iOS 特有的，别照抄 macOS 的清单就以为齐了** |

## build 号

`天.秒`（纪元日 + 当天已过秒数，零填充 5 位，例 `20683.07783`），从时钟派生、
**不从 git 历史算**。理由完整版在脚本注释里，三句话版：

- 原来是 `git rev-list --count HEAD`。2026-08 仓库重建，提交数从三千多掉到个位数
  —— build 号倒退，ASC 直接拒收，从新仓库一版都发不出去。根因是 build 号建在
  「历史」这个仓库里最脆的东西上。
- 不用 `YYYYMMDDHHMM`（12 位单段超 2^31，Apple 只规定字符数不规定数值宽度，不赌）、
  不用 Unix 秒（2038 越线，且它最大、会把「以后改主意」的门焊死）。纪元日最小，
  以后想换成任何更大的编码，闸都还放行。
- **不可读是有意的**（用户 2026-08-18 拍的板）。别以「可读性」为由改回
  `YYYYMMDD.HHMMSS`。可读时间脚本会打印出来 —— 抄进 TestFlight 的 What to Test。

事后反查任意一个 `<天>.<秒>`：`date -u -r $(( 天 * 86400 + 秒 ))`。

防倒退闸（`scripts/release/asc-highest-build.py`）是**三态**的，这条别退化：

- 拉到了、有 build → 比大小
- 拉到了、一个 build 都没有 → 真·首发，放行
- 够不着 API / 鉴权错 → **退出，不放行**

macOS 那条 appcast 闸曾经写成 `curl ... || true`，让「网络失败」和「首发」共用
一条放行路径 —— 网络抖一下，防倒退就静默失效。这里不重演。（瞬时故障有
有限退避重试兜底，4xx 立停。）

## 失败长什么样

| 现象 | 多半是 |
| ---- | ----- |
| `缺少 ASC_KEY_ID …` / `.p8 不存在` | 第 0 步就停了，凭据没配好，见上面「前置」 |
| `ASC_KEY_PATH 的文件名必须是 AuthKey_<KEYID>.p8` | key 文件改过名，`altool` 按名字找不到它 |
| `asc-highest-build: 重试 4 次仍够不着 App Store Connect` | 网络或 ASC 挂了。**这不是让你绕过闸的信号**，是「现在没法核实」——等恢复再发 |
| `App Store Connect 返回 HTTP 401` | key 失效/被吊销，或 issuer 填错。重试无用，闸会立刻停 |
| `build 号没有严格递增` | 系统时钟不对，或有人手工传过一个未来的 build 号。先弄清楚，别绕 |
| archive 报 `requires a provisioning profile` / `conflicting provisioning settings` | 签名参数没生效，或有人往命令行加了 `CODE_SIGN_IDENTITY`（自动签名下不能指定发布身份） |
| `产物是 ad-hoc 签名`（断言 ④） | 签名三件套没传到，产物用了仓库默认的 ad-hoc |
| `产物平台是 MacOSX`（断言 ③） | `-destination` 被改坏了 |
| `has entitlements that require signing with a development certificate` | 签名身份掉回了 ad-hoc（`-`）。iOS 上带 entitlements 不能 ad-hoc 签，见上面「签名参数从哪来」 |
| 一屏 SPM 包报 `Resources/PendingCrew.entitlements could not be opened` | 有人把 `CODE_SIGN_ENTITLEMENTS` 挪回了 xcodebuild 命令行 —— 那是全局设置，每个包都会拿这个相对路径去自己的 checkout 里找 |
| `源 entitlements 声明了 <x>，但产物里没有`（断言 ⑦） | provisioning profile 没授权这个能力，或键名用错了平台 |
| altool 报 `Successfully uploaded` 但 TestFlight 里没有 | Xcode 26 的 altool 从 bundle id 反查 app 时会挑到**相似**的那个（fastlane#29698/#29743）。脚本显式传数字 app id `6800257069` 正是为了这个，别把那个参数删掉 |

## 关于 altool 是不是过时了

**没有。** Xcode 26.5 里的 `altool` 是**重写过的新版**（`man 1 altool`，走 App
Store Connect API）；被降级成 legacy 的是旧那版，藏在 `--use-old-altool` /
`man 7 altool` 后面。man 页里上传的首选写法是 `--upload-package <file>`，
`--upload-app -f <file>` 是同义的旧拼法。

`notarytool` 是**公证**专用的，不传 App Store —— 那是 macOS 那条脚本的活，别拿来
发 TestFlight。

## 真上传之后

`altool` 返回只代表**传到了**，ASC 那边还要处理几分钟才会出现在 TestFlight 里。
脚本会把查状态的命令打印出来（`altool --build-status`）。

**上传是人的决定，不是脚本随手做的事**：`--dry-run` 跑通、产物验过，再去掉
`--dry-run` 重跑。注意重跑会**重新算 build 号、重新构建**（时钟又走了一段），
这没问题 —— build 号只要单调递增就行。
