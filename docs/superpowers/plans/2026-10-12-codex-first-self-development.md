# PendingCrew Codex 本机首版与自身开发实施范围

先交付能在 Mac 上用 Codex 共事的新版 PendingCrew，再在新版内推进完整工作模型、调度与其他端。2026-10-12 用户已决定这一顺序，并明确 Claude 经 ACP 也必须使用交互式 PTY，持续将提高质量与降低 token 消耗的经验纳入产品。本文落实 spec §27；它不代替当前在途单元的完整验收标准。

## 当前接线缺口

只读基线是保留整合树 `worktree-agent-a624116fb76233769` 的 `8c7d6efefb78f02c4f9ddca799c08b0848547617`，含另外在途维护单元。共享 main 为 `4c06b0eee394c6e91fcee9ed321a1c9185180a93`，不能把两个源码状态混作同一构建证据。

- M2a 八项终端核心任务和 M2b 十二项 Claude 会话驱动任务已有交付；后续安全按键、持久 human-stop 与编排接线已有范围受限的接受记录。Mac 终端消费与安装态完整验收仍有缺口。
- `crewd/crates/exec/src/codex` 已有 app-server 驱动；`server/src/lib.rs` 的生产 Executors 登记当前只有 `claude`，`orchestra/exec_claude.rs` 是现有生产适配器。Codex CLI 驱动通过不等于生产机组可以运行 Codex。
- `exec/src/codex/proto.rs::answer_server_request` 仍对漏到客户端的 Approval 返回错误、对部分 elicitation 返回 decline，并将 requestUserInput 归为 Unsupported。Codex 首版需要真实可操作的待请求链，不能把通知替代权限答复入口。
- 原始 Swift/Mac 客户端尚未切换到 crewd。上述判断来自源码；真实 Codex、GUI、安装和旧数据迁移不在本次只读核查内。

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
