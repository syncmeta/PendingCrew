# Mac 0.2.10：公开预发布验收

2026-11-08 01:25:18（台北）已在原仓库公开发布 [PendingCrew 0.2.10 预发布](https://github.com/syncmeta/PendingCrew/releases/tag/v0.2.10)。GitHub release id为`437723814`，`draft=false`、`prerelease=true`，发布时明确`make_latest=false`。本次先提供Apple Silicon Mac ZIP，未改稳定版自动更新渠道、Sparkle、Homebrew或其他平台资产。

## 准确来源与范围

版本为`0.2.10`，构建号`20764.60612`。轻量标签`v0.2.10`准确指向产品提交`e0feaefdde106a4d9aadbce417e7051569f0f378`；GitHub读取标签确认与实际App的`BuildStampCommit`、`PendingCrewSourceRef`一致。原main的153个已提交提交已普通快进推送至`3dccd99c710fc60704dbc3b1bfe741caee5b1019`，提交范围的秘密扫描通过。五份原有未跟踪设计文件和被忽略的私有交付材料未上传。

包内包含已通过相应模块验收的行政与待答问题整理、文档版本与工作关联、组织D/R/L及产物、11类表格字段与表／行生命周期、有界历史、十类对象搜索与一跳关联，以及真实Workspace组织表接线修复。它不包含仍在私有组合验证的通用查询、共享视图或记忆消费候选。各模块证据层级及剩余范围见[全工程核对](2026-11-05-project-progress.md)。

## 构建、签名与公证

从冻结的准确产品提交重新构建默认、非testkit的12个普通Core包，完整Release归档及Developer ID签名均退出0。签名Team为`M42BKJN82S`，使用安全时间戳，GUI和内嵌Core保持原签名与来源绑定。

Apple原提交`aa1ecf41-8574-4d82-bcc9-00295145efc6`只POST一次、原ZIP只上传一次。状态为Accepted；同id开发日志核对原输入，问题数为0。原公证输入保持不动，对新的准确App副本附票，执行严格签名、票据和无缓存Gatekeeper检查后重新打包，再解压复验。

公开附件：

| 附件 | 字节数 | SHA256 |
|---|---:|---|
| `PendingCrew-0.2.10-macos-arm64.zip` | 57,264,755 | `c543938deebd4af0802889a9dc889378db7e547d65fa3ca5d1e1fbd8f907d6d3` |
| `SHA256SUMS` | 101 | `60289349f48cd867fc5243f85160a776749121a95529721843d52cb0fa2302e7` |

签名Core SHA256为`d78a6c2080d4c12b991dd2b4ead339fe0f7aba605f8fd5529cdb64a4e1c3f671`；GUI为`32d848bcc57855a156fb779dd2a3ab8e817b19a11038d1bd6e5d857ec0de2761`。

## 公开下载复验

草稿内两个资产的GitHub服务端字节数与SHA256均先核对通过，再公开发布。发布后通过连接器读取公开release确认非草稿、预发布、准确标签与资产；另用无GitHub认证的HTTPS请求读取公开元数据，并实际完整下载ZIP及摘要文件。下载后的字节数、SHA256和摘要文件原文完全一致。

将公开下载的ZIP重新解压，核对App版本、构建号、双重来源字段及GUI/Core摘要；`codesign --verify --deep --strict`、`stapler validate`和不使用缓存的`spctl --assess`均退出0。公开包不是原始未附票的提交输入。下载复验未启动GUI或替换日用应用。

完整私有公证材料保留于被Git忽略的`docs/private/deliveries/2026-11-08-pendingcrew-0.2.10-20764.60612/notary/`。本次发布与匿名下载原始结果位于`/private/tmp/pcw-release-0210-onhkp1m5/`，总下载验收为其中`public-online-verification.json`，`ok=true`。

## 仍保留的验收

发布之后，公开0.2.10已通过正式33→33保护更新安装到实际日用，旧应用与完整数据均保留；正式组织表窗口的连接阻断已消失，实际建表、加行和同一行修改通过，详见[日用更新与窗口进展](2026-11-08-mac-0210-installed-acceptance.md)。表／行窗口历史及归档恢复、完整文档流程、真实Jev与Claude、原机组新任务、移动端真机、长期远程和跨端连续使用仍需继续。当前Linux普通后台已另行公开，见[Linux交付验收](2026-11-08-linux-current33-acceptance.md)，用户主机服务安装／升级回退仍保留。主工程继续推进。
