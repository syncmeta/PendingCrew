# macOS 发版 —— 怎么出一个能装、能自动更新的包

**一句话**：只跑 `scripts/release/build-macos-update.sh`，别手搓。

```sh
PENDING_NOTARY_PROFILE=pendingcrew-notary scripts/release/build-macos-update.sh
# 要顺带发到 R2（线上自动更新 feed）才加 PENDING_PUBLISH_R2=1
```

脚本自己钉 `main` HEAD 建干净快照 —— **工作区脏不脏都不影响所见即所装**。
它包办：构建 Release → Developer ID 签名（含 Sparkle 内嵌件）→ 公证 → staple →
生成更新说明 → `generate_appcast` 签 feed → 打 tag →（可选）发 R2，中间有六道
断言，每一道都对应一次真实踩过的坑（见脚本里的注释）。

## 挂 GitHub Release —— 和「发版」是两件事

**挂 Release 用手边那份已经公证好的产物，不为了发 Release 而构建新版本。**

这条写下来是因为它很容易被绑成一件事，而绑起来的代价是真的：`main` 上随时
可能躺着一批还没人工验收过的改动，为了挂个 Release 把它们构建成新版本，等于
把未验收的东西**推给自更新用户**（Sparkle 那边看的是 build 号）。Release 页面
要的只是「一个能下载能装的包」，那份包已经在 `dist/updates/pendingcrew/` 里了。

所以正常的顺序是：

```sh
# 1. 造 dmg（输入是已公证的 .zip 或 .app；dmg 自己也会签名+公证+staple）
PENDING_NOTARY_PROFILE=pendingcrew-notary \
  scripts/release/make-dmg.sh dist/updates/pendingcrew/PendingCrew-<版本>.zip

# 2. 把 tag 推上去（发版脚本已经在本地打过 tag 了）
git push origin v<版本>

# 3. 建 draft，两个产物都挂上
gh release create v<版本> --draft --title "PendingCrew <版本>" \
  --notes-file <发布说明.md> \
  dist/updates/pendingcrew/PendingCrew-<版本>.dmg \
  dist/updates/pendingcrew/PendingCrew-<版本>.zip
```

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
