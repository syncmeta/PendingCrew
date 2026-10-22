# CloudKit 启用提案（待根裁定，尚未执行）

产品源码基线 b38cbfdc377a14b5440b87f57cba44063590208b 已冻结；本提案只增文档，不改 Swift、工程、签名或云资源。个人同步底座与本机集成已源验，真实 CloudKit/双设备云读写尚未验。

## 具体配置与边界

| 项目 | 本次建议 | 已确认 / 尚需动作 |
| --- | --- | --- |
| 正常 Mac 与 iOS Bundle ID | `com.pendingname.pendingcrew`，两端共用 | 工程已有；不改名 |
| 产品 Team | `M42BKJN82S` | 真人已确认付费有效；真实证书 OU / profile TeamIdentifier 一致 |
| Crew 独立容器 | **拟用** `iCloud.com.pendingname.pendingcrew` | 仅提案；未查询或声称 Apple 端已存在；须在该 Team 核对可用性并新建/关联 |
| 首轮环境 | `Development` | 两端真实签名 entitlement 必须一致；配置枚举不能代替签名环境 |
| 数据库 / zone | 该容器的 **private database / default zone** | 复用现适配器；不建 public 数据、共享区或账号服务 |
| 真云资格测试 | `qualification.<同一个runUUID>.drafts` 与 `.hosts` | 两端显式使用相同测试 collection；只放有标记假原文/公开主机线索，不上传现有个人草稿 |
| 正式个人 collection | `drafts` / `hosts` | 资格后才由显式 opt-in 启用；不把测试结果自动推广到正式数据 |
| 隔离候选 | 已有源码 ID `com.pendingname.pendingcrew.native-candidate` | 若沿根既有独立测试 App 运行，需另核该 App ID 的 M42 签名/profile 并关联同一开发容器；不冒称远端已存在。正常两端 ID 不变，日用 App 不替换/启动 |

不使用 PendingNet、singbox 等其它产品容器。不复制设备私钥、client cert、grant、一次配对 token；目录只允许 displayName、`tls://host:显式端口`、transport authority/db/SPKI，仍须本设备独立配对。草稿内容 key 用实际 Core DataContext authorityId/dbId + crew/channel/原 session，与 Apple 账号分区独立。VPS/Mac 离线不影响个人草稿云层，但离线云目录不能授权或接管 Core。

## 四类动作分开裁权

1. **只读核对既有公开资源。** 已完成本机源码、证书 OU、profile TeamIdentifier 核对；不再重复证书扫描。以后在已登录的 Apple 开发者后台只读核对 Team、两个 App ID、拟容器标识可用性和已关联能力，属于下一次明确的账号服务读取。本次未登录、未调用账号/CKContainer。原 CN 后缀误标历史保留；Team 以独验真实 OU 为准（诊断 SHA ea161b61b6362499e5a337d662d41a0c03e21348e0ae0502ec705f325e7ecb3f）。
2. **新增/修改持久 Apple 资源。** 在 M42 创建 Crew 容器并为选定 App IDs 启用 CloudKit/关联；为实际 Mac/iOS 设备生成或更新对应 development provisioning profiles。既有有效同 Team Apple Development identity优先复用，不能默认新建证书；只有确实缺相应 identity/profile 才另报具体缺口。设备 UDID 若未注册，注册也是须明确授权的新资源。Development 建下述 schema/index；暂不部署 Production。
3. **源码接线与签名构建。** 在独立受审源模块完成下述账户切换/opt-in 接线，另设明确启用配置。正常默认仍 local-only。签名构建只用根已定 aeh 暖目录；实际检查最终包 Team、application identifier、container、environment 与 profile 相符，不能只看源 plist。候选构建不自动允许安装、账号读写或日用 App 操作。
4. **实际云读写与设备验收。** 另一次明确有界窗口，授权两台签名测试设备中的真实账户识别与专用 collection 读写。共享测试 UUID，不使用日常草稿或正式 collection。完成后按保留/清理裁定处理，不自动把 schema 或测试数据转生产。

## 必要 entitlement 与 schema

CloudKit 启用配置需在现有签名 entitlement 上正确加入：`com.apple.developer.icloud-services = [CloudKit]`、`com.apple.developer.icloud-container-identifiers = [拟定且实际已关联的Crew容器]`、`com.apple.developer.icloud-container-environment = Development`。application/team identifier、现有 Keychain groups 由正确 profile/签名展开并保原权限，不为同步扩大共享密钥组。不需要 iCloud Documents 容器、CloudKit Sharing、Push 或后台常驻：现适配器是明确调用的 exchange，无订阅/推送后台任务。

轻量偏好单独配置 KVS entitlement `com.apple.developer.ubiquity-kvstore-identifier`，正常两端拟 `M42BKJN82S.com.pendingname.pendingcrew`，须以实际 profile 校验；隔离候选使用其自身标识，避免改动日用偏好。仅 system/light/dark 与 compactRows 两个小字段，不存正文、整目录 blob 或身份材料。KVS 也在明确 opt-in 后才构造，非本次开发环境草稿交换的必要前提。

Development schema 精确复用当前 adapter：record type `PCWPersonalBranch`，`collectionID: String` 加 **QUERYABLE** index，`payload: Bytes`。recordName 由 account/container/environment/collection 分区与 branch UUID 稳定构造，读写 private default zone。仅按 collectionID 查询，不需 sort/search index；已知本机 IDs 额外直接 fetch，避免漏墓碑。payload 是完整分支或无正文墓碑；CAS `ifServerRecordUnchanged`，单条失败、容量、账号变化如实报错，无自动覆盖式重发。适配器现有每条 512KiB、交换 8MiB/10000条等边界需测试可见失败，不为本次扩存储平台。

## 待实现的源接线（不混当前冻结模块）

- 在原个人保存界面提供“启用个人 iCloud 同步”明确选择，先说明范围为个人草稿版本/所选公开主机目录/两项偏好。只有具备实际签名与配置且用户选择后，才构造 adapter、调用 accountIdentity；不做启动自动账号探测或 Core 连接/执行。
- 原 App 共享 owner 继续保证每 account+collection 只有一个 actor。把 synchronous latest 文件、memory/revisions、catalog、恢复与原 clear/merge tickets **全部按 account/container/environment 分区**，不能只切 actor 而让 latest shortcut 混账号。nil-account 资料原地保留；选择/退出账号不自动迁移旧账号或本机正文。显式选择哪些本机草稿/目录复制到当前账号，保留原本机分支，不自动上传整库。
- CKAccountChanged 立刻作废旧 generation、取消旧 exchange，先保全/导出当下输入再切分区；未耐久原文不能在账号切换中默默丢弃。清空旧账号可见投影，旧账号原文件保留、不得转入新账号。已有 selectAccount/transport cancellation 复用，不新建同步主库或 Core draft API。
- 提供明确“同步”与待同步/失败状态；只在真实当前账号 exchange 成功且无本地更新待发送时显示已同步。App 前台自动同步若后续启用，也只能在已 opt-in 的同一有效账号；无后台执行、任务重放、自动 claim/resume。另一设备版本只提示，不覆盖非空输入；选用/合并/退休沿已验全 payload+revision CAS 和显式确认。
- 关闭同步/掉账号回到 local-only 时保留原账户分区及未上传分支；只停止云通信，不自动拷贝或删除。再次启用同账号后明确恢复该分区，不能伪装为“云数据已清空”。

## 一次真实双设备验收范围

根安排签名隔离 Mac 与实际 iPhone/iPad（模拟 transport/普通模拟器不充真设备证据），两端同测试账号、同容器 Development、同测试 collection，用假草稿且不执行模型或向 Core 发消息：

1. 两设备各自产生独立安装 ID；同 key 同时写 A/B 不同原文、mentions、reply，双端真实 exchange 后两分支都在；非空输入不被远端替换。显式合并/退休可见，等字节 ABA 与晚回执仍不清新原文。
2. 断网/主机离线编辑与正常退出重开，latest-only 原文可离线发现。恢复网络只同步个人分支，不发送消息/启动或恢复 session；目录提示不提升授权。
3. 一端离线保旧分支，另一端明确退休后，旧端重连不得复活；离线后续原文进入本机 recovered originals，可查看/导出。
4. 真实账号切换由人类在设备系统完成，测试器不调用登录/登出；旧请求晚回不能混入新账号。新账号初始不见旧数据，原本机/旧账号仍保分区且可按正确身份重新打开。
5. 临时网络错误、服务拒绝/容量、CAS 冲突、不可用账号都保文并显示实际状态，恢复/重试只经明确个人同步动作；保存/导出、最终退出 guard 继续有效。没有真实云错误触发的项保未验，不用模拟结果替代。

保留：测试 collection 的 runUUID、分支身份/回执状态与脱敏证据；本机原文和失败输入不自动删除。停止 opt-in 不删云记录。测试区清理另经明确裁定，先停两测试端并导出，再仅处理本次 collection；不得删容器/重置整环境或移除正式墓碑。墓碑不能随意 GC，否则旧设备可能复活。个人云与 Core 业务日志不双写，业务权威始终 Core。

## 根可一次裁定的最少人类步骤

建议一次确认：使用 M42、拟独立容器 `iCloud.com.pendingname.pendingcrew`、首轮 Development、正常 App IDs 与是否增关联隔离候选 ID、专用假草稿 collection、允许的具体两设备与账号读取/写入窗口。若后台已有当前登录，可在该窗口完成资源核对/创建与 profiles，不要求人类重复填开发字段；遇到真实登录/二次验证或设备系统账号切换才由人类操作。生产 schema 部署、发布签名、安装/替换日用 App 与正式草稿上传不在这次提案授权内。

Apple 一手依据：CloudKit 能力/容器与项目配置见 [Enabling CloudKit](https://developer.apple.com/documentation/cloudkit/enabling-cloudkit-in-your-app)；实际环境由签名 entitlement 决定见 [CKContainer](https://developer.apple.com/documentation/cloudkit/ckcontainer)；能力与 KVS entitlement 见 [Configuring iCloud services](https://developer.apple.com/documentation/xcode/configuring-icloud-services)；Development 验证后另行部署 Production schema 见 [Deploying an iCloud Container’s Schema](https://developer.apple.com/documentation/cloudkit/deploying-an-icloud-container-s-schema)。本次仅检索公开文档，未访问开发者账号控制台或云服务。
