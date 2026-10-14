# 原前端正式配置与最终交付单元（方案，尚未实现/验收）

本记录只安排剩余完整产品交付，不能把候选准备、开发分支整合或CI-only main提交当正常安装完成。已验来源与业务复用原记录。原Mac窗口、通讯录/消息/群聊/会话视图及原iPhone/iPad导航是唯一产品路线；独立Native工作区只留迁移验证用途。

## 完整正式配置

- 普通正式Mac启动默认选中一个crewd authority；不要求用户手敲`--crewd-ui`。保留原视图，旧LocalBackend/白板/SessionHost执行腿不作为失败回退。候选编译条件仍只用于隔离验收，不靠改默认配置解决AppKit注册上下文问题。
- 正式本机后台数据与presentation目录使用稳定命名空间，不随commit SHA或候选版本变化。初次创建严格owner/private/noalias，既有不可信目录不自动chmod采纳。具体正常路径和旧数据切换在14正常helper与全源码组合冻结时落实；读旧JSON导入属于显式迁移，旧账不得自动成为第二写者。
- 正式配对保留真实安全持久vault与可信TLS/SPKI/grant链，电脑/authority身份绑定。candidate memory-vault只用于fake隔离，不能当普通重启后保持配对。实际读取个人vault、安装、日常数据切换分别核已有人类动作授权。
- iPhone/iPad同一可信remote authority与稳定私有presentation。退出/后台只关闭本connection/watch/lease，不停止远端私人团队；原手机导航、配对Form及签名邀请→回应→确认码三阶段保留。unsigned SDK构建不代真机。

## 14客户端完整接口组合（等待后端唯一writer全模块冻结）

接正式`command.submit/lookup`、typed首消息原子命令、另一个session-start receipt，与已有document/folder/remote特定requestId/CAS共存。不能把本机UUID/unknown snapshot或文本相似视为服务器受理证明。原authority/payload/generation保留，断线unknown只读确认，不跨crew replay。

终端复用原会话呈现：正式inspect/watch的真实VT或structured runner来源分别显示；Human明确claim lease后才有input/resize/release，partial/unknown字节不自动重发。仅真实PTY广告raw能力，Codex structured runner不假造PTY。`written`只表示内核接受的精确字节，不冒称TUI消费或命令执行完成；有限流gap后snapshot与在线撤权为完整判据。

Stop/Resume/structured approval依正式receipt、same session/thread与实际新generation。关窗口保留团队；显式Quit只对始终核对的own child、真实kernel IPC peer UID/PID/birth/boot和可信server runtimeBinding关联提交hibernate并等实际退出。断联/未知所有权不杀历史daemon，remote只撤自身。现normal13缺14 runtimeBinding时ownedQuit pending，不以JSON PID/path赋权。

## Apple完整交付判据

最终冻结源码+正常helper完整fresh-own graph+明确正式构建配置形成新不可变包。wholeGUI输入、SPM前后全量、settings/log/SDK/signature与helper原compiled来源逐一保存；source/test-only变化不重标旧normal，也不因此重跑无影响稳定业务。

原App首次启动在新私有scope核具体bundle/source/binary/helper/fake/env/kernel身份；初始化→正常后台→原群聊/Todo/文档/进展→成员/审批/终端→authority切换/草稿→unknown重启确认→窗口常驻/明确Quit按整块验。OS启动上下文或App权限阻断单独记录，不能修改业务根或换壳逃避。实际窗口、真机、真实模型、普通安装、日常数据切换与公开发布互为独立证据层，按各自动作授权完成。

## Linux最终可靠运行边界

当前受管单元仅同prefix/data持久lease下的安装/版本切换/unknown/CAS/旧版预执行拒绝/停态独立快照backup/restore。旧11原binary不改，其raw未来版拒绝仍可能清空侧文件；受管入口保source/target全部physical/mode。此时任非空WAL/journal保留并failclosed，是安全拒绝，不是自动crash恢复。

最终正常同版本crash→安全恢复须接14官方WAL副本/commit-cut验证与实际同版恢复或明确受管恢复入口。不能用人工删WAL或raw绕manager代替。支持能力应由精确冻结生产来源与实际门禁证明绑定，不以schema数字猜“版本足够新即可跳预检”。实际普通Linux运行核是Mac本地控制无法替代的必要质量；其余来源/拒绝/静态已足证范围不重复派CI。无默认systemd/login item或真实系统安装。

## 冻结与权限

后端拥有Rust/schema/公共DTO/正常helper构建；本线拥有Swift/工程、candidate/正式recipe、平台安装/分发。公共构建文件唯一writer，global整合由主负责。先形成可审的完整生产组合与候选证明，再核实际安装/真实数据/发布/新模型动作授权。main用户dirty、原候选/日志/DB/binary/SDK证据始终保留；Jev、多后端个人记忆与团队知识仍只方案，不计已完成。

## 2026-10-14：已定来源上的可执行交付准备（尚未执行）

本节追加43b既定单元的具体动作/来源绑定，不重置已验业务。原前端14适配source-review为6715e3c47516ddee1803f86e23b095b8cba04419，Native读者准备为1d2e19e14e2f583d23f17c92e36cfb0f44785e52；两者仅开发源码，实际App仍07/5cdf+2bf。最终整合必须记录新GUI完整commit、后台actual compiled commit、全部Rust输入digest与默认feature/profile/target、原始CargoJSON及fresh-own graph、actualbinaryhash、GUI编译条件/settings/SDK/SPM前后全量、包签名。任何后续安装/设备动作只引用该不可变manifest和实际路径，不只引用分支名。

### 正式配置与候选隔离的证明recipe

1. 同一最终源码导出不可变snapshot，使用既有DD/SPM。候选构建使用实际`PENDINGCREW_NATIVE_CANDIDATE`专用条件；ordinary构建的actual `SWIFT_ACTIVE_COMPILATION_CONDITIONS`必须明确不含该条件。分别保留完整`xcodebuild -showBuildSettings -json`及actual build argv/log；不能用候选settings证明ordinary。
2. 根据冻结`CrewdFrontendConfiguration`确认candidate SHA目录与ordinary稳定`PendingCrewCore` sibling的实际分支；普通入口无参数即原App/Core。私有测试root只能由明确`PENDINGCREW_CREWD_DATA_DIR`指定，所有别名/宽权限拒绝依旧。源码验证不替代下一actual ordinary隔离启动；启动仍需root核具体bundle/hash/private-env。
3. candidate凭据factory必须实际返回memory，并确认所有配对入口均由同factory获得，不能仅HOME隔离。ordinary凭据factory实际返回正式Keychain，其稳定service为`com.pendingname.pendingcrew.crewd.remote.v1`、`kSecAttrSynchronizable=false`；不读取旧PSK service。显式fixture memory覆盖不能被当ordinary持久化证明。
4. 持久vault真实验收准备：使用单独明确fake service（`com.pendingname.pendingcrew.delivery-validation.<freshUUID>`），假的签名配对材料与正常TLS/SPKI链，不采用真实service、不列举全Keychain。A进程通过真实`CrewdRemoteKeychain(service:)`只写该fake record；退出后B进程只读该同record并完成同authority确认/连接；再撤销或同身份恢复后验证旧信任失效，最后仅删除该fake service自己已记UUID。必要记录为record UUID、privatekey/certificate内容的hash、API status、service namespace、A/B真正进程/来源、后台实际grant/authority；不打印secret原文。这个真实Keychain动作尚未执行/授权，不得从memory跨进程失败或普通编译通过推断持久化已验。
5. 真生产service/日常配对凭据不参与上述验证。接真实个人配置时，读/写哪些record、要批准哪个App安全提示，由root按具体动作核已有授权。证据中保留API原始失败码与准确context，不能通过忽略Keychain错误或切回memory使ordinary表面可用。

### Publisher正反门禁的实际recipe

- readonly包准备命令可审结果：对**最终ordinary实际App路径**运行`codesign --verify --deep --strict --verbose=4 <app>`、`codesign -dvvv <app>`、`codesign -d --entitlements :- <app>`，记录源码manifest/实际binarySHA、identifier、Team、CDHash、runtimeflags以及是否get-task-allow；证书/账号标识在面向用户报告中仅必要范围展示。此前07只能candidate adhoc/exact，不能填作publisher-positive。
- 正例需要确实使用已选择的DeveloperID身份，原release recipe已有`ENABLE_HARDENED_RUNTIME=YES`。签名身份/Team由具体批准的构建输入给出，不从本地默认adhoc或任意Team字段猜。操作在下一授权后只制作可审查包，不顺带公证、上传或公开发布。
- 正例运行：初始可信Human anchor通过闭合local.clientTrust的publisher参数注册**实际正在运行的ordinary App**；服务端动态OS验证sealed `com.pendingname.pendingcrew`、批准Team/Apple DeveloperID requirement、hardened与no-debug。退出该App后新PID只读恢复原receipt并保持一般命令资格；新App不得hibernate非其拥有的历史daemon。真实kernel/birth/boot与运行SecCode才是门禁，保存JSON/PID/文件路径本身不赋权。
- 同批反例：07 adhoc candidate申请publisher；同identifier不同Team；同Team错误identifier；缺hardened；get-task-allow；任意Python/Terminal仿claimedPID；旧候选CDHash变化；grant撤销/restore后新PID。每个负例保留真正签名/进程身份、拒绝类型、receipt/全部授权表零效果及活跃watch/lease关闭证据，不能用缺文件/错误JSON代替真实签名负例。没有合法Team/证书或App权限时列具体阻断，继续其它独立模块。

### 设备/安装/日常数据切换动作清单

| 动作 | 明确输入与许可 | 成功证据 | 失败/回退边界 |
|---|---|---|---|
| 新候选原窗口验收 | 最终immutable GUI/helper/manifest + 新0700 scope、fake profile、具体一次启动context | actual新PID/binarypath/hash、kernelhelperbirth/peer、原布局受影响流程和14receipt/terminal/unknown | 保原07/822/703；不复用旧GUI、不杀旧helper、来源/启动context失败不伪产品CRUD通过 |
| iPhone/iPad安装/运行 | 精确已签device build、entitlements/profile、实际可连接设备和具体授权 | 安装产品hash、真实系统版本/运行日志、原IPadShell导航/配对/remotescope/退出只关闭自身 | unsigned SDK不能充真机；不更新真实配对、不凭手机本地路径猜服务器文件 |
| Mac候选安装 | 明确安装目的地、当前App版本/是否在运行、精确新签名bundle和其内normal14 | 原目标bundle完整保留、同volume staging/hash验证、原子activation/readback、实际新PID | 未获授权不操作Applications；失败保原App，不替换其它用户副本/旧helper |
| 稳定Core新数据启用 | 指定稳定目录，旧legacy与新Core路径/owner/mode/DB身份，明确允许哪些旧数据读入 | 初始/切换manifest、正式Core备份、import真实结果与authority/DB变化、未知原请求保留 | 不自动列举daily根、不自动import、不同时开旧账写者；无实际data授权只演练隔离副本 |
| 已有Core13到14 | 正式normal13备份+明确db/id/WAL/SHM/完整hash与目标helperproof、drain停态 | 正式migration/rollback/reopen与请求见证、14 crash/WAL恢复现有独证按影响复用 | 不改PRAGMA/删WAL；物理/未知差异保留，旧13不直接打开14库 |
| 安装或数据回退 | 明确旧bundle与其真实maxschema、原备份来源/DB identity、正式Core恢复命令 | 先恢复可开schema的独立快照，核全部physical/logical/unknown历史再启动旧bundle | 只换GUI指针不等DB回退；old raw future-version副作用仍保限制，不能为回退绕managed preflight |
| 最终发布 | 精确正常安装/设备/模型等已验范围与具体发布授权 | 实际签名/公证/上传/release来源与hash | 不把开发branch push当发布，不把候选adhoc/unsigneddevice当release |

实际操作前均准备source-qualified action manifest，列唯一目的地、原状态、完整输入/产物hash、data范围、ownedPID/birth、可回退旧产物/备份以及读取/写入/停止/安装的具体动作。主代理审该可执行manifest并核授权；权限提示只暂停确实依赖它的步骤。该计划不产生后台请求、不读取个人凭据、日常DB或真实模型，也没有新CI。
