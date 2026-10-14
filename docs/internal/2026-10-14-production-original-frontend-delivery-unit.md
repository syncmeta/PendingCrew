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
