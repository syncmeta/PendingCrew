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
