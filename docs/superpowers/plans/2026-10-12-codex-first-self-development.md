# PendingCrew Codex 本机首版与自身开发实施范围

先交付能在 Mac 上用 Codex 共事的新版 PendingCrew，再在新版内推进完整工作模型、调度与其他端。2026-10-12 用户已决定这一顺序，并明确 Claude 经 ACP 也必须使用交互式 PTY，持续将提高质量与降低 token 消耗的经验纳入产品。本文落实 spec §29；它不代替当前在途单元的完整验收标准。

**产品定位（2026-10-12 人类补充）**：PendingCrew 主要面向个人，目标是一个“专业可靠，又了解我”的私人团队。本体、记忆、知识与表服务于个人交代工作、团队协作和持续学习，不以企业化管理为默认目标。Codex 本机首版先可用；这些后续能力由新版 PendingCrew 来开发，不增加 C1 至 C3 的前置工作。

## 当前接线缺口

### 2026-10-12 推进记录

- 按人类授权，降噪改动单独移植到 main，`d3cb5b23` 已推送并核实远端；该基线通过格式、lint 和 1115 项离线测试。整合树另外 207 个提交未随降噪带入 main，共享目录中既有未跟踪 ACP 资料保留。
- Jev 多后端仅保留[讨论方案](../specs/2026-10-12-decision-provider-options.md)，不继续实现。
- Codex 首个接线单元保存为独立候选 `codex/codex-orchestra-20261012@36f2ac81`：新增独立 Executor 适配器，传递世界观、模型/effort、MCP、持久 thread、上下文、因果绑定和停止/续跑；成功投递复用已有观察函数，在 app-server 确认接受后、广播事件前登记。回归先在未接观察器时失败，再验证修复及超时不计成功；候选源码通过格式、lint 和 2320 项离线测试（8 项活测试未运行）。此处不是生产或真实 Codex 验收。
- 首版候选已推进为 `codex/codex-orchestra-20261012@9424c207`：生产 `serve --codex` 接线、Item/TurnEnded/Profile/Notice、原生人工请求与 Mac NDJSON 操作窗口已实现；MCP 默认招募能沿用实际 Codex 机长。完整后端 2333 项通过（8 个活测试未运行），Mac 完整复查 3228 项、12 项跳过、零失败，Mac/iOS Simulator 构建与原生客户端合成验收通过。独立 ad-hoc 本机候选已构建；合成窗口已验证新建、发消息、原生审批/答复、恢复与停止。用户随后授权独立首版真实任务，现有 Codex 登录已完成一次隔离自我开发回归，群内报告与红/绿测试已核查。未安装、未迁移日常数据，也尚未将候选代码纳入共享 main。本机运行产物对应 bec60f86，eeb24dca 仅增加回归测试。详细边界、首次测试失败与最终结果见[候选记录](../../internal/2026-10-12-codex-first-local-candidate.md)。
- 下一步按这个顺序串行推进：核对并纳入已接受后端整合输入；完成 Codex 生产事件与待请求链；接通 Mac 本机候选并验证恢复路径。完整自由表、Jev 自动调度、其他端与 Claude ACP 不抢占这条路径。

最初只读核查的基线是保留整合树 `worktree-agent-a624116fb76233769` 的 `8c7d6efefb78f02c4f9ddca799c08b0848547617`，含另外在途维护单元；当时共享 main 为 `4c06b0eee394c6e91fcee9ed321a1c9185180a93`。这是历史核查快照，不能把两个源码状态混作同一构建证据；本次进展见上面的推进记录。

- M2a 八项终端核心任务和 M2b 十二项 Claude 会话驱动任务已有交付；后续安全按键、持久 human-stop 与编排接线已有范围受限的接受记录。Mac 终端消费与安装态完整验收仍有缺口。
- 最初基线的 `serve` 只登记 Claude；候选现已登记独立 Codex Executor。共享 main 与候选仍是不同源码状态；不能把候选验证写成 main 或已安装应用的运行事实。
- 最初基线对人工 Approval/requestUserInput 使用兜底拒绝；候选现已保留原始请求并提供精确答复入口。尚不支持的通用 MCP 表单有明确拒绝与通知；真实账户、窗口实际操作及日常迁移继续单独验收。
- 候选新增 crewd 原生 Mac 入口和独立数据空间，原始 Swift/Mac 日常入口与旧数据保持现状。上述范围区分源码、合成验收与实际运行；真实 Codex 的首个隔离任务及窗口操作现已验收，安装和旧数据迁移仍未完成。

## 按依赖推进的四个单元

### C1 Codex 进入生产编排

复用现有 start_codex 与 SessionHandle，新增 Codex Executor 并登记 `codex`。对齐现有 Executor 契约：世界观和 MCP 参数、登记先于就绪、持久 thread ID、send_bound 的因果来源与成功投递观察、Item/TurnEnded/Profile、打断、停止与进程收尾、续跑失败的诚实状态。不得把 Send 入队或正文写入当作 Delivered，也不得编造传输不支持的逐消息回执。

先用 fake_codex 和真实 Core/DbPort/serve 接线验证新建、消息、上下文、工具通信、停止后重启不复活、恢复同一 thread、迟到事件和来源绑定；再跑受影响的共享执行器、游标预留、因果链和生命周期回归。真实 Codex 验收是后续独立阶段。

### C2 Codex 待请求与 Mac 操作路径

保留 runner 请求 ID、session/turn 身份和原始选项，统一呈现权限、工具提问及支持的 elicitation。人答复只能回到对应仍有效请求；断线、cancel、turn 终结、超时与重复答复有明确状态。保持 runner 的真实权限边界，不自动批准，也不将未知请求永久悬挂。

Mac 先实现机组/session 导航、消息与结构化工具进展、发任务、待请求、停止/打断/恢复、连接健康和后端版本提示。通过 crewd API/事件流访问数据，沿用单一写者；连接丢事件须能重新同步。首版不把 Claude 屏幕/按键分支作为前置条件。

### C3 安全候选与自身开发验收

完成当前在途单元，串行整合已接受输入，生成组合 schema，再构建本机候选。迁移先用合成数据/明确授权副本验证快照、幂等、回滚、缺库与身份检查。授权后才切换真实应用与数据；隔离开发实例不能与日常实例同时写同一数据根。

首个真实验收任务使用小而实际有价值的改动：人在新版发任务，Codex 调用机组 MCP、实现改动，独立审查与验证，回报产物和剩余边界。验收同时覆盖人工权限答复、停止/恢复、客户端重连、后台失败的可见性和不重复投递。保留旧应用与数据恢复路径；一次成功任务不等于全部工程或所有故障场景完成。

### C4 工作与成本视图逐步进入产品

先让已有任务、计划、消息和产物有稳定引用，展示负责人、当前结论、阻断、下一步和证据链接；后续接统一工作账本与对象模型，不建临时第二本权威账。

每次开发单元采集 provider 原始 token 字段，区分新输入、缓存创建、缓存读取、输出、reasoning 与未知值。分别记录实现、审查、验证解释、协调和返工。以“完成并接受的单元”对比成本，不以会话忙闲或测试总数衡量产出。

第一批改进实验：机器生成完整证据索引与异常摘要；短当前状态配历史按需读取；修后独立审查增量；按影响范围选择定向/跨层测试；相同证据一次保存多处引用。每个实验同时检查缺陷捕获、漏检、返工和持续时间；成功经验进入 spec 与产品上下文管理，失败经验保留适用边界。独立审查、原始日志、最终完整门禁和敏感边界验证继续保留。

## Claude ACP 的并行兼容范围

先核查 [claude-code-cli-acp](https://github.com/moabualruz/claude-code-cli-acp) 与 [claude-agent-tui](https://github.com/zeo-workspace/claude-agent-tui)，固定通过核查的发布版本，本地注册为 `acp:<adapter-id>`。crewd 做通用 ACP client，适配器必须在真实 PTY 中运行交互式 Claude CLI；不能以基于 SDK 的 claude-agent-acp 或 `claude -p` 静默替代实际执行。PTY 桥本身不增加模型调用，结构化内容优先取 Claude hooks/transcript，屏幕判据用于确实缺乏事件的交互。

用假 ACP agent 验证 JSON-RPC 双向请求、能力协商、MCP、流式更新、permission 回传、cancel/迟到完成、外部进程断开和恢复。loadSession 不支持就显示能力缺失，不假报已恢复；重放消息不得再次触发投递/唤醒。usage、终端、配置与子 session 能力分别协商，不能因“支持 ACP”就推断全支持。

选定版本的源码/协议审计和离线契约通过后，再做实际 Claude CLI/账户/工作区验证。必须复测人工输入保护、迟到回执、stop/resume、退出后的子进程收尾与版本漂移；ACP 名称本身不能替代这些验收。PTY 必须保留，原始 TUI attach 的客户端展示另按需要实现。ACP 不阻塞 C1 至 C3，也不要求先完成所有通用扩展。

Codex 优先沿用现有 app-server 驱动，并核查 [codex-acp](https://github.com/agentclientprotocol/codex-acp) 对 permission、usage、模型/effort/fast、MCP 与子 session 的映射；是否改为 ACP 根据能力与迁移成本决定，不因统一协议就重写已接受基础。更详细的分层与成本风险见 [协议与 token 审计](../../internal/2026-10-12-protocol-and-token-audit.md)。

## 首版与后续工程的边界

完整自由表、本体、自动 Jev 调度、行政巡检、移动端、跨网配对、Linux 安装更新继续保留，但不阻塞本机 Codex 首版。基本权限隔离、消息可靠性、人工停止、恢复语义、数据库耐久性和明确错误提示属于首版必需质量。

文档中的发布顺序决定已经生效，功能仍按上面单元实现和验收。main 合并、push、正式发布、安装、运行实例和真实数据切换分别记录授权与证据，不从“尽快用上”推断所有外部动作均已完成或获批。

## 新版可用后的私人团队研究候选

以下为文档与部分源码调研，不是选型定案或运行验收；先用新版开展小范围比较，不提前实现新记忆后端。来源核查日期为 2026-10-12，实际试用应固定版本。

- **Engram**：单 Go 程序、SQLite/FTS5、MCP，agent 主动保存精选经验，先搜短结果再读全文。优先比较跨 runner 的轻量持久记忆。其 Codex hooks/runtime 身份接入不能直接当成 PendingCrew app-server 已兼容；先核对裸 MCP 与身份契约。[架构](https://github.com/Gentleman-Programming/engram/blob/main/docs/ARCHITECTURE.md)
- **QMD**：本地全文、语义检索与重排，适合作为资料查找组件；先关键词，确有需要时再用本地模型。默认嵌入模型偏英语，中文/中英混合资料需与多语言模型比较，记录下载大小、内存、延迟和索引更新行为。[项目](https://github.com/tobi/qmd)
- **Basic Memory**：Markdown 笔记与关系，适合用户查看、修订、迁移自己的知识；与 Engram 比较写入体验和读回质量，不同时启用两套重复摘要/注入链。[项目](https://github.com/basicmachines-co/basic-memory)
- **OpenViking**：重点借鉴摘要/概览/原文三级加载、个人/成员记忆和资料/技能的统一入口；完整运行需要 embedding 与 VLM 等处理，不从分层读取推断整体省 token。[项目](https://github.com/volcengine/OpenViking)
- **Hindsight、Mem0、claude-mem**：保留自动提取/反思路线，先测新增模型调用能否换来更少纠正、返工和历史重读，不默认逐消息或逐工具调用整理记忆。[Hindsight](https://github.com/vectorize-io/hindsight)、[Mem0](https://github.com/mem0ai/mem0)、[claude-mem](https://github.com/thedotmack/claude-mem)
- **Grist、Graphiti、Cognee、Letta、OpenSPG/KAG 等**：按出现的实际需求借鉴自建表、关系与时间、代码知识和成员持续学习；完整平台与图数据库不成为私人团队或 Codex 首版的必要依赖。

私人团队首先需要记住用户明确偏好、理解正在做的工作、积累可复用经验，并在依据不足或方案有问题时诚实指出。个人偏好、工作现状、经验教训和推测分别标明；新决定替代旧决定，原始来源可读回，用户可查看、修订和删除。知识来源与验证状态是辅助信息，不变成负责人标完成的门禁，也不新增产品审批。可复用的工作流程逐步成为 skill。

试验至少覆盖：明确偏好跨会话保持、旧决定被替代、中文/中英混合检索、多个成员同时更新、无关项目不被误带入，以及删除后的检索与同步副本处理。比较原生记忆基线与新增组件，记录正确/错误回忆、需要用户重复解释与纠正的次数、完成质量、返工、总 token、索引/摘要/检索耗时。检索找到了相似文字不代表记忆正确；了解用户不等于迎合用户。

## 通用 agent 通信优先，厂商内部协作延期

2026-10-12 用户明确补充：先不做 Codex 内部通信，先以通用通信为主，预留不同厂商 agent 之间的协作。此前对原生子 agent 的研究只是资料，不进入当前实施范围；不把 spawn/message/wait 映射加入首版前置。

- PendingCrew 的 session ID 是成员身份，crew/channel/message ID、回复关系、提问、投递记录、停止和恢复属于 crewd 的通用契约；runner 的 thread/turn ID 只保存在执行器绑定及原始证据中，不能替代团队成员身份。
- 成员通过现有 crew MCP 发帖、提问和读取团队消息。接收由通用唤醒/投递链按 session 路由；每种适配器只转换自己的输入、权限请求、进展和结束事件。消息层不能要求双方同厂商或共享同一厂商代理树。
- 沿用现有来源、因果绑定、去重、投递观察和故障恢复规则，不新建第二本通信账。不把入队、写入、runner 接受和工作完成合成一个成功状态；能力不支持须明确返回，不能模拟厂商回执。
- 后续 Claude ACP/PTY 和其他厂商仍接这条公共链。协议适配与厂商原生临时子 agent 的管理分别演进；目前不额外引入通信协议或模型中转服务。

候选已补同一 crew 下假 Claude/PTy 与假 Codex/app-server 的实际驱动混合验收：通用 message.post 按指定 session 投递，未指定成员不被唤醒，接收者的投递记录收敛，客户端断开重连后消息不重复、两种执行器不重复提交，停止后自有进程登记清空。Codex/Claude 的七个集成回归通过，固定 Rust 1.94.0 格式和 lint 通过；注入重复消息的临时坏例失败，原始测试恢复后通过。此项证明混合执行器的通用接收与路由；消息来源为测试客户端，不能声称已经完成两个真实厂商 agent 经 MCP 双向对话的验收。日志见 `/private/tmp/pcw-mixed-executor-{replay-red,restored-green,clippy-final,final-check}.log`。共享夹具原有无效默认更新语句在固定工具链 lint 中报错，已移除并复验；初次失败日志 `pcw-mixed-executor-clippy.log` 保留。

首版下一项仍优先验证后台失败/重启后的可见状态与恢复，不扩展记忆、Jev 或厂商内部通信。已完成的真实任务和窗口验收记录见[候选记录](../../internal/2026-10-12-codex-first-local-candidate.md)。


## 2026-10-13 持续推进检查点

Codex 首版已补后台重启人工恢复入口与休眠后停止快照读取；正常/强制退出、人工停止、同 thread 恢复、五个同步边界及实际合成窗口按钮已验收。候选与 main 已接受降噪/方案已在隔离树整合，保留有界批处理与双方交付记录。完整门禁、首次失败、稳定产物及工具阻塞记录在候选记录；候选仍不是安装或日常迁移。2026-10-13 已将 `29e72bb3` 快进合入共享 main，既有 README 未提交改动按哈希核对原样保留，ACP 未跟踪研究资料保留。下一步核对 M4b 工作账本持久化/工具/编排接线缺口；Jev/记忆继续讨论先行，厂商内部通信延期。
## 2026-10-13 M4b 工作身份基础

工作引用基础位于 codex/work-identity-20261013。数据库 v5 增加全组织唯一的对象引用编号、类型、所属机组和源定位；只持久化身份，不复制工作状态。前缀沿用原始需求 #、计划 W、待人事项 H，以及已约定的 D/L/R/O/T。对外需要显式文本 ID DTO，不能把 i64 编号当 JavaScript number 传输。

新需求/计划/问题创建的真实账本事务会绑定 work: 命名空间的源引用；旧本地别名保持兼容。绑定失败与账本/公告一起回滚。已有公开引用、旧问题元数据不能替代丢失的账本；身份不一致、已发号计数回退、既有对象被命令静默移除都拒绝，不自动清空或重绑。机组移除后保留读取，永久删除随机组级联清除对象引用；提交过的编号由持久单调序列保留，不因最高号删除而复用。编号耗尽明确拒绝，不绕回。

先红后绿：pcw-work-identity-red-01.log 捕获普通主键复用最高号；pcw-work-ref-ask-red-01.log 捕获未接入的实际创建事务；pcw-work-ref-identity-scope-red-01.log 捕获另一机组账本被错误接受；pcw-work-ref-missing-book-red-01.log 捕获缺账被猜成新账本。8 项存储身份/迁移边界、27 项问题规则定向回归已通过。v4 迁移前快照保留，既有源状态不改写，失败仍是 v4。部分旧测试固定版本 4，已改为当前版本并保留数据断言与迁移前版本校验。

完整门禁过程中发现既有真实 PTY 测试设置阶段偶发失败。新增“回执先可见、字节后到”的确定性反例捕获了旧夹具提前认定屏幕到达；夹具现在确认实际解析字节到齐，逻辑时钟一致。生产私有排空路径继续使用真实时钟，不向 RPC 或权限策略开放时钟/观察接口；25 项反压/安全写入回归通过。日志 pcw-pty-frame-witness-{red,green}-01.log；初次全量失败及定向结果分别保存在 pcw-work-identity-backend-check-02.log、pcw-work-identity-pty-focused-01.log。

这只是 M4b 基础，不是完整全局工作模型、工具面或界面已交付。历史对象引用补齐、可读代号/永久旧别名、工作动作 API/MCP、检查点/停下即检查接线、工作视图继续推进。已发现 mark_done 仍要求 evidence，而人已决定完成凭据可选；下一单元对齐该规则，保持负责人权限及图不变量。所有日志位于 /private/tmp，不运行活测试，不开始 Jev/记忆实现，不安装或迁移日常数据。

经验：用实际身份、实际字节和事务提交做见证，避免“进程写完/忙闲/有文件/已入队”充当完成证据。确定性反例先把不可靠前提固定下来，再复验完整门禁；不靠反复重跑或降低阈值放行。

Final offline gate: 2349 passed, 0 failed, 8 ignored; fmt/clippy passed. Evidence: /private/tmp/pcw-work-identity-final-check-03.log. Runtime migration/source-reference changes remain on codex/work-identity-20261013 until the next integration checkpoint.
## 完成凭据可选规则对齐

按 §24.1 的现行决定，工作负责人自报完成可以不附凭据。mark_done 接受可选凭据，旧 Evidence 参数调用继续兼容；缺省保留 None，不生成虚假的“已验证”文字。传入空白凭据仍拒绝，其他负责人、未完成子项、依赖关系和终态规则继续生效。行政角色事实关闭他人工作仍要求凭据；负责人本身不因此增加验收门禁。

先红后绿：pcw-work-optional-evidence-red-01.log 证明旧不变量拒绝无凭据完成；工作核心新增负责人/他人权限、子项、解阻、恢复及事件兼容回归。历史 done 事件的 W 字符串 ID 和已提供凭据保持原编码，缺省凭据省略并按 None 读回。第一次事件夹具误用数字 ID 的失败保留在 pcw-work-optional-evidence-green-01.log，修正后工作核心通过 pcw-work-optional-evidence-green-02.log。

本单元不改变事实性放弃/行政关闭的证据约束，不实现 Jev/记忆，不安装或启动真实数据实例。后续继续历史引用补齐、工作动作 API/MCP、持久工作事件和检查点/停下即检查接线；各单元以真实事务、源码和运行证据验收。

Optional-evidence gate: 2352 passed, 0 failed, 8 ignored; fmt/clippy passed. Full log: /private/tmp/pcw-work-optional-evidence-full-check-01.log.

## 2026-10-13 M4b 工作历史持久化

数据库 v6 增加真实工作事件表。命令更新的账本快照、事件、对象引用与公告在同一单写事务内提交；事件写入失败整笔回滚。事件按原始序号/行为者保存，不覆盖重复序号；查询限制为 1–200 条，并校验记录序号与事件内容一致。迁移只给旧账本补已有对象的全局引用，包括已移除机组；不改变工作状态，不合成迁移前历史。旧账本身份/不变式无效时迁移拒绝，版本与部分引用一起回滚。

实际问题创建命令的旧实现未留下事件，反例失败记录 pcw-work-history-integration-red-01.log。定向存储与问题集成分别通过 pcw-work-history-store-green-02.log、pcw-work-history-integration-green-02.log；补充分页、重复序号整笔回滚、记录身份篡改和后段迁移失败后完整门禁通过 pcw-work-history-full-check-01.log：2361 passed、0 failed、8 ignored，格式/lint 通过。首次编译和旧版本夹具错误保留，不算行为反例。日志位于 /private/tmp。

此单元仍位于 codex/work-identity-20261013，不代表 M4b 全部完成：公开工作 API/MCP、进展检查点、空闲挂钩和工作视图仍待接线。没有新增真实模型调用、安装、发布或日常数据迁移；Jev/记忆继续仅保留方案。

## 2026-10-13 M4b 全局计划读取接口

增加 work.list/work.get 客户端 RPC、严格参数与生成的 v3 增量契约。列表支持全组织或单机组，默认 50 项、最多 200 项，稳定全局编号游标分页；同页同机组账本只恢复一次。移除的机组照样可读，账本缺失或身份不一致拒绝，不猜空工作。编号/修订在线上使用字符串，父项、依赖、原始需求和卡点都转换为全局对象引用；机组内别名不充当公开编号。新增稳定拒绝码与中英文文案。

列表只返回工作摘要，长目标、验收标准和凭据从 work.get 按需读取。实际 Unix socket 验收确认分页、全局父项、摘要不夹带完整目标、详情保留目标，以及错误参数的拒绝码。故意把链接投影成局部 W1 的坏例在期待 W9007199254740993 处失败，日志 pcw-work-query-local-link-red-01.log；恢复后定向通过 pcw-work-query-focused-03.log，摘要改动后通过 focused-04.log 与 socket-01.log。

查询接线完整后端门禁 pcw-work-query-full-check-01.log：2364 passed、0 failed、8 ignored。之后摘要投影与新增 socket 夹具的最终变更以格式、全 workspace lint、proto 全测试、全部 socket 测试及 CLI 全方法端到端复验，日志 pcw-work-query-final-01.log；没有把改动前的全量数冒充改动后的完整重跑。首次两次编译错误是工作目录/夹具类型错误，不计作行为反例，原始日志保留。

当前接口仅为计划读取，不代表原始需求/待人事项全量视图、操作命令、MCP、跨机组关系、可读代号、自建表、检查点和注入已经完成。下一单元接工作操作与 agent 工具，沿用同一账本、全局引用及事务。没有新增真实模型调用、安装、发布或日常数据迁移。

## 2026-10-13 M4b 工作操作事务

增加 work.create/work.update 客户端 RPC 与严格输入契约；创建支持目标、验收层级、负责人、同机组的父项/依赖/原始需求引用和检查时间。引用先从全局编号定位到权威账本，绝不拿其他机组的同号别名代替。更新支持进展/下一步与完成；完成可不附凭据，提供的凭据仍须非空。每次更新必须带读取到的修订字符串，比较和修改处于同一个单写事务内，旧修订返回稳定冲突码，不覆写新进展。

调用者角色按当前机长指针、管理机组机长及归属重新判定；其他机组的机长不自动成为目标机组机长。工作项负责人仍按核心权限改状态；冻结机组和冻结的调用者机组拒绝写入，保留读取。真实 agent 工作操作把 note_progress(WorkItem) 与状态、事件、引用一起提交，失败整笔回滚；提交成功才发布 work.changed，通知只带全局 ID 与机组，不广播长正文。

六项事务行为验收通过 pcw-work-actions-focused-03.log。实际 Unix socket 验收通过 pcw-work-actions-socket-01.log：创建后通知、无凭据完成、旧修订冲突且状态不变、非法完成层级拒绝。pcw-work-actions-revision-red-01.log 实际观察到移除 CAS 后旧备注覆盖新备注；progress-red-01.log 观察到移除真实进展写入后错误提交。恢复正确源码后完整门禁日志为 pcw-work-actions-full-check-01.log。最初冻结测试错误期待 Forbidden，而既有规则正确返回 Conflict/crew.removed，改为检查原稳定码；不算产品缺陷反例。

本单元不是整个 M4b 完成。MCP 工具、带修订号的我的工作摘要、依赖放行后的唤醒、检查点/IdleHook/上下文注入、跨机组关系、可读代号与自建表仍需继续。当前跨机组关系明确返回未接线拒绝，不静默错连；公开完成命令已放行依赖状态，自动跟进还未接线。候选分支继续开发，未切换日常应用/数据、未新增真实模型调用。

Final gate: 2372 passed, 0 failed, 8 ignored; fmt and workspace clippy passed.

## 2026-10-13 M4b agent 工作工具

MCP 增加 my_work_items、work_item_get、work_item_create、work_item_update，直接调用与客户端相同的全局工作事务，没有模型中转或第二本工作账。工具参数、嵌套凭据对象严格，不能从参数冒充 human actor。权限由实际 session 与当前机长指针重读；修订号使用文本，并从摘要/上一回执直接复用。

my_work_items 跨机组筛选当前成员负责的项，默认只列活项并排除已移除机组；公开列表默认仍可读全部对象。显式 include_finished/include_removed 可查看历史和冻结项，摘要与详情带 frozen 标志，避免把只读旧工作当成仍可推进的任务。缺账/缺对象不能被 owner/live 过滤掩成空列表。新增工具数为 worker 21、captain 32；这会增加初始工具声明成本，不能据减少正文就声称整体 token 节省率已证明。

创建和更新的 MCP 成功回执也只给摘要；目标/验收标准/凭据详情通过 work_item_get 按需读取。实际 helper 反例 pcw-work-mcp-echo-red-01.log 捕获创建回执重复长目标；改为摘要后 e2e-green-02.log、e2e-frozen-03.log 通过。真实 helper + 合成数据库/会话控制验证建项、摘要带修订号、按需详情、无凭据完成、历史/冻结筛选、旧修订和伪造 actor 参数拒绝，未调用真实 runner/model。负责人分页跨机组、冻结只读、缺账拒绝定向通过 frozen-04.log；所有日志在 /private/tmp。

首轮窄 server 测试没有工作区启用的 testkit，既有 s1 测试编译失败，改用 --lib 定向入口；catalog 注解列表与角色工具数量按新增四项更新，未削弱权限检查。full-check-01 在文件布局 lint 失败，已将生产端口移到 test module 之前。full-check-02 因磁盘 ENOSPC 编译失败；确认进程停止后仅删除本轮 /private/tmp/pcw-decision-target/debug/incremental 可重建缓存（目录占用 23GB、实际恢复约 14GB），保留依赖、源码、验收日志与应用/原型产物，并以 CARGO_INCREMENTAL=0 继续。full-check-03 发现 RPC 多行数量断言仍是历史 17/28；更新为 21/32，rpc-final-01.log 32 项通过。

full-check-04 捕获既有 S1 权限夹具的就绪发布竞态：文件已存在但 PID 内容还是空的，尚未执行权限判定。生产权限规则未改；假 runner 先完整写 staging 文件，再原子 rename 发布就绪。新增确定性创建阶段回调测试，提前暴露最终文件的坏例 pcw-s1-readiness-red-01.log 失败；正确实现 green-01.log 通过，全部六项真实 socket/注册/重父化权限测试 authority-green-01.log 通过。最终完整门禁为 pcw-work-mcp-full-check-05.log。可复用经验：就绪边界必须是完整有效数据，不是文件刚创建；修正夹具的因果边界能同时减少偶发重跑成本并加强验收，不能降低权限断言来消除偶发失败。

下一单元为有界、稳定的我的工作上下文和检查点/停下检查接线；跨机组关系、可读代号、自建表和行政巡检仍待完成。前端由用户设计，独立 Sketch 原型已交付 design/prototypes/2026-10-13-private-team，18 画板、151 条原生 Flow；原生文件解析/导出与浏览器 13 个流程通过，本机 Sketch.app 损坏，未把原生播放当成通过。该原型未提交/推送；后端继续候选分支，不擅自实现用户正在设计的界面。Jev/记忆仍方案先行，无新增真实模型调用/安装/发布/日常迁移。

用户随后明确纠正前端：保留原来的设计与微信式形态，不做总览/早上好；左侧仅通讯录（原层级）与消息（原时间流）两种视图，选中机组后右侧消息（群聊/单聊）、Todo、文档、进展四视图。第一版仪表盘草图撤回为设计方向，仅保留历史文件、不覆盖用户可能的修改；同一前端子代理已按 owning Sources 重做微信布局版本。界面实现等用户设计对齐，后端继续。

Final gate: 2375 passed, 0 failed, 8 ignored; fmt/workspace clippy passed, CARGO_INCREMENTAL=0.

## 2026-10-13 M4b Codex 工作上下文接线

生产 serve 默认 ContextExtras 已从空桩接到专用 Db 线程读取全局工作账本；显式 testkit hooks 保持调用方配置。只取当前 session 负责、未完成/未丢弃、未冻结机组的项，每页最多 20 项，渲染结果最多 4096 UTF-8 字节。标题/下一步/阻塞列表裁剪均自报，容量不足保留真实 nextAfter，修订号始终为文本。长目标、验收说明和证据不常驻；需要时使用现有工具读取。

每个摘要使用稳定字段，不加入当前时钟；工作清空仍发送明确空列表，数据库关闭或账本损坏返回 unavailable，不伪装成无工作。当前沿用 Codex additionalContext keyed/untrusted 协议，未新增另一层客户端上下文缓存，也未测量真实模型总 token 节省比例。挂钩外层超时仍沿用编排既有回退规则，精确缓存/压缩后保留行为尚不能由合成 runner 证明。

发现并修复首次启动缺口：旧挂钩只在已启动会话 send_batch 调用，首回合 LaunchRequest 没有上下文。新增执行器首回合能力判断和 initial_context；Codex 显式支持，首回合与后续回合都走相同不可信数据入口。其他执行器不主动收集这段上下文，Claude PTY 的 UserPromptSubmit 接入仍待后续。

本机 codex app-server generate-json-schema --experimental 无模型调用生成的 schema 再次确认 additionalContext 与 kind/value 字段。真实模型已读白板的旧证据见主设计 §23，不把本轮合成协议请求扩称新的真实模型验收。首回合丢弃上下文反例准确失败，日志 pcw-work-context-adapter-red-01.log；恢复实现后首回合协议、编排能力判断、真实数据库负责人/完成/冻结筛选、UTF-8 容量/分页/修订号、稳定空列表与错误区分均通过。

这一单元仍非完整 M4b：检查点/停下即检查、人回复与工作依赖放行的自动唤醒、跨机组关系、转派与全局工作模型继续推进。前端设计已交接独立 session「PendingCrew 前端设计」，保留微信式原结构，后端不越过用户设计讨论。无新增真实模型调用、安装、发布或日常数据迁移；Jev/记忆继续方案先行。

完整 crewd/scripts/check.sh（Rust 1.94.0，offline，incremental=0）通过：2381 passed、0 failed、8 ignored；fmt 与 clippy 全目标通过。完整日志 /private/tmp/pcw-work-context-full-check-01.log；此 SHA 尚未合入共享 main，也未重建/安装桌面候选。

## 2026-10-13 M4b 阻塞解除持久唤醒

人类待确认被回答或既有默认方案到点后，直接把 BlockerCleared 里的各负责人唤醒写进同一 collab 事务。沿用 AskAnswer 的原来源键和人类优先级；多项归同一负责人只入队一条，负责人同时也是直接提问者时与原提问者唤醒去重。附言仅列全局 W 号，最多 50 项并明确 total/partial，不复制长目标或证据。缺失 session 不让人答复失败，已归档目标由既有 wakes::enqueue 跳过；没有擅自恢复或转派失联/归档负责人。

work.update 完成一项时也把显式 BlockOn::Item 解除后的负责人提醒纳入同一事务，走 System 优先级。普通 dependsOn 边另比较真实前后状态：仅最后一个未完成依赖完成、工作已无其他卡点时提醒；不制造 BlockerCleared 历史事件、不改写历史。终态重复完成仍由核心拒绝，过期 CAS 不产生第二条提醒。

真实 SQLite 测试覆盖多项/多负责人/跨机组负责人、原提问者合并、没有事件观察器仍留下 pending、归档和已删除 session，以及队列写入失败时答复/完成状态、被放行项、历史事件和先前已入队行全部回滚。原始终态测试误预期重复完成成功，核心实际返回 Terminal；已修正为验证既有拒绝与队列无增量，没有放宽核心规则。丢弃全部负责人提醒的反例准确失败（旧行为仅 1 条，要求 3 条），见 /private/tmp/pcw-work-unblock-red-01.log。

这是持久入队及事务证据，不扩称本轮新增实际 runner 投递或真实模型回合验收。完整 M4b 仍需：检查点到期合并与真正投递后的 missedChecks 记账、停下即检查的三次抑制/行政交接、过期工作提醒在投递前重新核对当前负责人/终态、释放项 work.changed 客户端通知，以及其他工作动作/跨机组关系与客户端视图。协作事件漏调不再影响这两类提醒入队；既有 AskAnsweredHook 继续保留给后续观察能力，不重复造提醒。

继续候选分支开发，未切换共享 main、安装应用、发布或迁移日常数据，未新增真实模型调用；Jev/记忆继续讨论先行。前端由独立设计 session 负责。

完整 crewd/scripts/check.sh（Rust 1.94.0、offline、incremental=0）通过：2386 passed、0 failed、8 ignored；fmt 与 clippy 全目标通过。完整日志 /private/tmp/pcw-work-unblock-full-check-01.log。

## 2026-10-13 M4b 自动工作提醒过滤与认领保护

工作完成产生的 System 提醒在渲染时重新核对权威账本：只保留仍属于该负责人、未终结、未冻结的工作项；全部过期则进入既有 unusable/drop 路径，不生成模型回合。普通 System 与人类 AskAnswer 契约保持原样，人的答复不被自动工作过滤器吞掉。

新提醒在持久队列附言中保存完整 targets，渲染给模型前移除这份派发元数据，仅展示最多 50 个当前有效全局 W 号和准确 total/partial。不能仅凭最初展示的 50 项都完成就认定整条无效：第 51 项仍有效的反例已覆盖。读取按机组缓存账本，避免每项重新恢复同一本账。旧未截断记录可以核对；旧 partial 记录没有完整目标，保守保留，不将缺失身份视为完成证据。异常元数据、缺失权威账本或读取失败原样返回错误，保留提醒供重试。

渲染与认领之间也有事务保护：Snapshot 保存这条工作提醒实际渲染的简短正文，DbPort 在同一数据库作业捕获；claim_reserved 在认领队列/游标租约的同一事务内再次核对。正文因负责人/终态变化而失效或改变时返回 Stale，队列仍 pending、attempts 不增加，下一轮重新渲染。正常重渲染后可认领；claim_output 同样经过这条保护。没有新增第二套投递账本。

解除显式卡点和普通依赖变为可推进时，事务内也发布对应全局 WorkChanged；不以新增 WakeEnqueued 或旧本地 W 别名替代客户端工作变更通知。

验证覆盖完成、强制换负责人、归档、删除机组、缺失 work_books、旧记录、普通/人类提醒隔离、完整目标裁剪与不回显、认领前并发变化及通知集合。合成归档测试最初违反“有活子机组不能归档”规则，修正为先归档子机组，没有放宽生产规则。两个反例分别让过期提醒继续渲染、移除认领核对，均准确失败；日志 pcw-work-followup-filter-red-01.log、pcw-work-followup-claim-red-01.log。首次门禁指出 collapsible_if，按现有 lint 修复，没有 suppress。

这是 SQLite/渲染/实际认领事务的证据，不扩称新增真实模型验证。后续仍要接检查点与投递后的 missedChecks、停下即检查/行政交接，并补实际 actor 在渲染屏障后的竞态回归，以及将来修改依赖/新增卡点时普通 ready 提醒的进一步资格过滤。人回复通知保留，不能为节省 token 擅自忽略仍卡在其他事项上的负责人。

候选分支继续开发，未合入共享 main、安装、发布或切换日常数据；Jev/记忆仍只保留方案，前端由独立设计 session 负责。

完整 crewd/scripts/check.sh（Rust 1.94.0、offline、incremental=0）通过：2390 passed、0 failed、8 ignored；fmt/clippy 全目标通过。日志 /private/tmp/pcw-work-followup-filter-full-check-02.log。

## 2026-10-13 M4b 检查点队列与投递记账

新增 checkpoints：从非归档、确有到期项的已有工作账本筛选，以 WorkBook 的 due_checkpoints 为权威判定，按全组织负责人合并（包括跨机组负责人），经既有持久 wakes 入队，Checkpoint/System 优先级。每名负责人已有 pending/inflight 检查点时不再造新提醒；相同目标/修订/检查时间的终结清单在既有队列保留期内不重建，避免失败后反复生成新的提醒身份。空扫描不创建工作账本。生产 DbPort 已接 Core::on_tick，合成 MemPort 保持默认空实现。

队列保存完整 checkpointTargets（全局 W 号、当时修订、检查时间）；渲染只给模型最多 50 个当前仍到期的 dueWork 号和 total/partial，移除派发元数据。当前负责人、修订、时间、冻结、终态/卡住与未回报上限均通过当前账本重新核对。继承上一单元 Snapshot/认领事务保护，捕获和认领都使用调用方提供的时间，不在 collab 读取墙钟。

关键边界：入队、认领、忙碌排队、投递失败均不增加 missedChecks。只在实际 ack_delivered 的同一事务内记录 CheckpointWake 并顺延复查时间；租约重放/重复成功确认不再记第二次。若提醒准备后负责人已经更新工作修订/检查时间，旧确认不覆盖新回报。记账写入失败时，已观察到的原投递命运与 inflight/租约一起保留供同一收据重试，不启动第二个模型回合。

六个真实 SQLite/租约测试覆盖跨机组合并、排队不计数、失败/作废不计数、旧回报竞态、记账失败整体回滚与原租约重试，以及两次成功投递后停止继续检查。另用真实 Hub、DbPort、运行中的编排 actor 和合成执行器验证 Tick→持久入队→派发→原成功收据→只记一次，重复收据只保留一个含该 W 号的回合；正常退出关闭自有 actor/DB。移除 ack 记账反例在实际 actor 链路准确失败，日志 /private/tmp/pcw-checkpoint-receipt-red-01.log。

此证据是数据库与合成运行链路，不是新增真实 Codex/Claude 模型回合或正式客户端交付。missedChecks 达既有上限后由纯模型 owner_lost 判定的后续失联通知/交接、停下即检查的三次抑制/行政交接、相关工作动作和客户端视图仍待继续。检查点操作本身会改变工作修订，严格 CAS 保持；后续需要让过期修订回复携带精简的当前事实，避免 agent 为一次冲突重新读长目标。

未安装、发布、切换日常数据或新增真实模型调用；Jev/记忆保持讨论先行，前端由独立设计 session 负责。

完整 crewd/scripts/check.sh（Rust 1.94.0、offline、incremental=0）通过：2397 passed、0 failed、8 ignored；fmt/clippy 全目标通过。日志 /private/tmp/pcw-checkpoint-full-check-01.log。
