# 原生完整体验打磨 — 源码冻结交接

## 来源与边界

从已接受/已安装前端的 `c78c9b6ddb49db999f054e74cd3b248760906cbd` 开始，普通 rebase 到最新发布证据 `5d32b32e021e99477e1950581b4320293b892ec7`。分支 `codex/mac-demo-polish-20261022`，完整方案 PLAN.md 已获根接受。真实 demo 来源、TSX/CSS 和最新 desktop/mobile 图片逐文件 hash 在 DEMO-BASELINE.json；公开版本 d2805167。未更改 design 作者文件。

原 B 单链接模块保留于另一分支 checkpoint `f0a81c203f133effbe01a6e4aaf21d10be16101d`，未混入本模块。Cloud 账号接线、APNs、Rust、SQLite/schema/API 均无变更。已装日用 App ee9e/20748.47322 未启动、退出、替换或操作。

## 完整落地

- 共享 presentation：平台字体、表单分区/字段、header/footer、主次按钮、可见状态与展开技术明细。Mac 13pt正文，iOS系统动态字体/44pt触控；深色语义色保留。
- Mac 日常 MacThreePaneView：原生主侧栏+内容+可收放 inspector；无选中机组时不再重复右侧巨大空态。窄窗右栏转同内容 sheet，常规宽度内收放；主侧栏系统恢复按钮与会话列表自己的收起/恢复入口分别保留。聊天实例/查询/草稿不因选择另一内容面卸载。
- 通讯录/会话：浮动材质主侧栏、移除正常 crewd 的装饰 lineage bar、稳定头像/标题/摘要节奏、紧凑搜索与工具区。两端共享真实会话数据。
- 待办工作：五个既有顶级内容入口显示为消息/人待办/AI待办/文档数据/计划进展，原内部 section key不变。独立人工/AI inspector，详细去正确类别。来源状态/负责人/标签分层；全局源属性与本群标签仍使用原各自 CAS/Unknown gate。
- 群角标：只复用当前 crew/channel 已完整读取、无失败且非 loading 的无过滤 channelWork 快照；AI Requirement 排 done/closed，人工 HumanTask 仅 open，manualState 优先。零隐藏，未加载未知不冒0；不拿未读/mention/plan/page.total/tagCounts/owner当提问者计数；零新增 badge read、缓存或全机组扫描。实际原 refresh 的 params 仅 channelId/limit/after，各分页原 revision/total校验未变。
- 文档：本群引用/原文来源分层，长正文有阅读宽度；新建文档明确保存/加入本群两阶段，原阶段 UUID/source恢复、Unknown lookup、失败保留原文函数不变。引用选择器单一纵滚；自由 Web renderer 未换成模板。
- 计划：真实层级与状态继续使用原 planTree；标题、状态、负责人、说明、检查点分层，表单明确目标与验收字段，无假百分比。
- 创建机组/成员：名称/目录/第一件事层次清楚；原文首任务与独立名称分开，高级权限折叠；真实能力说明与未知提交仍可核对。新成员参数区可滚动，不把长 Codex 设置挤到输入区外。
- Codex：概览/执行/历史/设置四个界面分区，保全部原命令/确认/receipt/可用性与结构化审批表单；观察/期望/Unknown不混，原 JSON仍有次级查看入口。手机长生命周期操作按完整行排列。
- 连接：原三文件稳定协议不变，增加当前阶段/身份/确认层次与可见失败状态，不引入B授信流程。
- 退出：只改说明层次/进度字体；直接退出/安全暂停/叫停、可关闭进度、force及未耐久输入精确revision闸全部保原语义，没有继续后台选项。
- Composer：保现 IME/键盘/附件/草稿及发送回调；固定 UI 用“待办工作”，用户内容不做替换；iOS label 内的发送命中区与附件类入口至少44，保原30pt发送图形。
- 额度：黑 OpenAI标识与Claude归属明确，正常 crewd没有真实额度feed则显示“额度未读取”；不画假环或拿会话token冒订阅额度。

## 本轮新增的真实 iOS 入口（不冒称旧版已有）

实际 `IPadShell → CrewChatView → CrewdOriginalWorkIntegration` 增加新成员 sheet、本群“待办工作与成员”sheet，成员点击复用已有 `IOSRemoteSessionView`。原成员详情现在直接提供共享 `CrewdCodexSessionControls`，绑定原 workspace/current target/权限，零 appearance启动/恢复。人/AI inspector点击详细关闭sheet并切正确类别。iPad宽屏列表+正文，手机/窄分屏会话列表与内容返回；两端主侧栏仍系统原生恢复。

这些是本轮接线，旧 C 源的备用 mobile workspace 控件不作为旧日常入口已具备的证据。

## 已执行验证

最终完整 Mac Debug arm64 / LOCAL_MAC+REMOTE_PRODUCT 与 iOS ordinary Debug arm64 Simulator 串行 xcodebuild 均 exit0。同原 aeh DerivedData/SourcePackages，禁止自动包解析/更新，jobs2；没有新冷缓存、SDK或CI。前后388项 Swift/工程/lock输入相同，准确 argv、log/hash见原作者树私域：

`docs/private/preserved-delivery-artifacts/frontend-polish-20261022/full-target-validation.json`

Mac log `mac-target-freeze.log`，iOS log `ios-target-freeze.log`；早期各轮日志保留。这里只是全目标源码编译门，未封候选/zip/签封、未boot/install/launch。Mac编译输入 helper准确标为历史ee9e/37c8，不重标为本轮来源。最终 normal/helper 与完整包须由根按接受后的完整输入另冻。

独立设计评估 `/private/tmp/pcw-design-quality-polish-20261022/{design-quality-spec.md,wip-source-review.md}` 的源码布局项已收敛：同轴嵌套滚动、44pt命中、创建底栏提示换行、手机Codex长按钮。它不是 native GUI 通过证明。稳定A/C/同步持久化与Core证据复用，不因视觉更改重跑后端业务。

## 下一道准确候选 GUI 门（尚未执行）

必须用本轮正常产品实际入口、有内容的隔离真实数据；不以空机组空窗截图交付。根/QA协调准确新normal、标准Mac产品身份及普通iOS候选后验：

| 流程 | 必验内容 |
|---|---|
| Mac 1440/1100/900 | 长机组/会话标题与消息正文；主侧栏及会话列表隐藏恢复，搜索/选择/输入仍在；宽→窄→宽、手动隐藏→窄→宽、窄sheet开着放大再关，专门核inspector系统Binding回写行为。 |
| iPhone约390 / iPad768/1024 | 真实IPadShell列表→群→人/AI待办→文档→计划，sidebar恢复；新成员与Codex实际入口；动态字号/键盘下按钮可达，发送label命中至少44。 |
| 待办/知识 | 人工0隐藏不冒已加载未知，红黄范围只当前真实群；人/AI详细各归本类；文档长正文与两阶段状态/失败原文保留界面。 |
| 创建/控制 | 长路径/参数/模型高级设置可滚动，首任务与名称区分，未提交不开会话；控制的期望/实际/未知及结构化请求仍可识别。不得为视觉验收启动真实模型。 |
| 状态/退出 | 只读、离线已缓存/草稿、loading、失败下一步、未耐久保文/导出与停工进度/关闭/force；重开App零自动session。 |

上述 GUI 与最终安装/发布均未由本作者执行。本模块先交完整 source + 两端 full-target 冻结，不能把本次编译产物冒准确候选。实际数据/KeychaininstallID/签封路径由最终候选manifest固定，沿原双env隔离（PENDINGCREW_CREWD_DATA_DIR才是Core根，DATA_DIR只legacy），不启动备用workspace或改purpose冒隔离。
