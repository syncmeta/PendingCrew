# 连接恢复与运行环境切换模块验收

准确产品输入为 `7042e7b1171f4be160a5a0e6193f3bee91e4e332`，已在完整模块验证后普通快进合入 main。九份生产文件和两份专项测试文件均与冻结复核输入一致。现有 README 编辑与五份未跟踪设计文件逐项摘要保持，私有查询／行政候选未混入。

本次完成的是凭据等待、取消、运行环境选择及跨窗口失效的完整模块。操作系统同步凭据读取未返回时，超时／取消只结束本次观察，原底层操作继续占有唯一读取槽，不谎称已经结束，也不在其后排队重复读取。用户可以返回本机。新环境必须通过原凭据、真实传输身份、hello、grant 与原提交历史验证后才替换旧环境；失败和迟到结果不覆盖新选择。已明确完成的凭据删除／替换及时停用匹配旧连接，原草稿和结果未知的提交仍保留供核对。

普通 iOS 保存电脑列表沿实际 IPadShell 入口接入进展、取消读取／选择和返回。弹层拥有独立标识，旧弹层关闭或迟到回调不能取消新选择。主机目录保存是非权威的后续工作，不能阻塞已完成身份和历史验证的连接。

## 实际验证

| 层级 | 结果与边界 |
|---|---|
| 实际普通 Mac App 模块 | Debug 正式 PendingCrew scheme 完整构建通过；24项专项通过。同步读取通过显式释放的受控阻塞核对；没有以自身定时返回代替真正挂起。真实 Client、Workspace、Journal 执行就绪、历史及后端发布门。 |
| 原会话回归 | 在同一冻结 App 动态库上38项全部通过，保留原提交／恢复／结果未知处理边界。 |
| 真实加密组件 | 原 Apple 短链接登记、丢失 commit 回复后的原请求 lookup、真实 mTLS、原巡检回执在新连接查回、在线和新连接撤权通过。 |
| 实际 App 与真实 Core 组合 | 使用实际 App 的 Window、AppModel、Workspace、Journal，两个窗口分别本机→真实 mTLS 远端→本机；后端作用域、原 namespace、原草稿、原 Unknown 均核对。一个窗口返回本机不影响另一个已验证远端。无模型任务。 |
| 普通 iPhoneOS 构建 | 正式 PendingCrew scheme、Debug、arm64 完整构建通过，包含实际 IPadShell 与 IOSRemotePairingView；不据此宣称真机或手势已验。 |
| 独立复核 | 冻结 v3 全部11路径及原安全依赖复核通过；保留原失败、修复和对照记录。 |

Mac 构建及组件组合使用真实公开普通33代 Core。其准确来源仍为 `e0feaefdde106a4d9aadbce417e7051569f0f378`，与本 Native 输入的已提交 Rust 产品树同为 `f07fc41880b2e9df35f48c055dd08d33741851f8`；没有把旧 Core 的 hello 标签改成本次 Native 提交，没有使用私有查询 Core。组件 Core 使用原隔离工具拥有的 SIGTERM 加 wait(exit=0) 清理；这不是 safePause 三标记自然退出证明。

此前跨窗口已知凭据变化、取消后的列表通知、独立凭据拥有者待连接和 iOS 弹层关闭均曾暴露边界缺口，修复后纳入24项对照。真实加密工具最初缺少客户端已经使用的行政协议依赖，另一次 Root 自选临时目录不符合原夹具前缀而在连接前被拒绝；保留两次失败，补入准确生产依赖并遵循原目录限制后通过，未放宽认证断言。

## 证据与尚未交付范围

统一准入记录：`/private/tmp/pcw-root-native-selection-7bm2l9zq/complete-v3-main-admission.json`，SHA256 `4a857a427b68351dcb25fed6ee857015fcf821eab0d77b0f94323a1f4eb3d3ea`。

主要证据为同目录 `app-build-v3/result.json`、`q8/results.json`、`q9/results.json`、`ios-build-v3/result.json`、`main-integration-v3.json`；实际加密组件在 `/private/tmp/pcw-enrollment-patrol.native-v3-20261108-01/`，实际 App 双窗口组合在 `/private/tmp/pcw-enrollment-patrol.native-app-v3-20261108-01/`。独立源码复核清单 `/private/tmp/pcw-native-selection-v3-independent-review-aqpec71g/manifest.json`，SHA256 `9355b9b619264f68a0fc5bded3b8c7121bb83375edc745ea3327a0d15b52c887`。

本次没有签名、公证、替换日用应用或发布新包；日用与公开 Mac 仍为0.2.10，Joy 已安装后台仍为公开33代。新版正式窗口、iOS 实际手势／真机连续使用及后续安装交付分别继续验收。行政完整策略、通用查询与组织记忆消费、跨平台及长期运行仍属于全工程未完成范围。
