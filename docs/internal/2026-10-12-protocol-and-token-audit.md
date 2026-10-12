# PendingCrew 协议接入与 token 消耗审计

Claude 经 ACP 也运行交互式 PTY；Codex 本机首版优先接完整生产链。ACP 可以把 PTY 控制封装成统一接口，但不能消除适配器内部对输入、权限、取消、恢复和终端版本漂移的处理。降低 token 的主要机会在信息选择、唤醒理由、状态变化与可复用证据，协议翻译本身不应调用模型。

本次为 2026-10-12 源码与设计审计。Swift 取共享 main `4c06b0ee`；crewd 取保留整合树 `8c7d6efe` 加在途维护修改。检查涉及 world/render/inject、wake plan/policy/breaker、Codex proto/session/turns、编排默认 hooks、旧 HookEmitter 及相关设计章节，未逐行审计全仓、未运行真实应用或模型，不把历史 usage 当现行浪费量测。

## 协议层与执行方式

| 路线 | 执行方式 | 客户端获得的能力 | 局限与用途 |
|---|---|---|---|
| 直接控制 Claude TUI | crewd 持有 Claude PTY | 原始屏幕、按键、resize 与可自定的状态接口 | 自己维护 CLI 版本、交互、权限、投递和恢复；已完成基础可复用 |
| Claude PTY 经 ACP | 外部桥持有同一种交互式 PTY | initialize/session/prompt/update/permission/cancel 等结构化接口 | TUI 适配责任转给桥，不会凭协议名称消失；原始 attach 与工具终端不是同一能力 |
| Codex app-server | 官方结构化进程协议 | 持久 thread、turn、事件、请求与模型配置 | 当前 PendingCrew 已有底层驱动；按官方版本生成契约并维护兼容 |
| Codex 经 ACP | codex-acp 启动 app-server 并映射 | 共享 ACP 客户端与各项协商扩展 | 少一套自有协议映射，但新增依赖/翻译层；需核对已有能力不退化 |
| Claude stream-json | 官方 print/headless 路径 | 结构化输入输出 | 不满足用户指定的交互式 PTY 执行约束 |
| MCP | agent 调工具与资源 | PendingCrew 通信神经、读取与动作工具 | 不替代完整 session/turn 生命周期；继续与执行协议并用 |
| A2A | 跨服务 agent 任务交换 | 任务、状态、产物、异步更新 | 更适合以后跨服务组织协作，不负责 PTY 输入/原始终端控制 |

SSH、WebSocket、stdio 和 Unix socket 是传输/进程控制方式，不能独立定义“这一轮完成”“这个权限请求还有效”或“消息已送达”。首版用当前 app-server + Claude PTY 桥接 ACP + MCP，不再增加第三套通用执行协议。

来源：[ACP](https://agentclientprotocol.com/protocol/v1/overview)、[Codex app-server](https://learn.chatgpt.com/docs/app-server)、[codex-acp](https://github.com/agentclientprotocol/codex-acp)、[Claude CLI](https://code.claude.com/docs/en/cli-reference)、[MCP](https://modelcontextprotocol.io/docs/learn/architecture)、[A2A](https://a2a-protocol.org/latest/specification/)。codex-acp 的旧 zed-industries 仓库已提示迁移到 agentclientprotocol/codex-acp，不沿用旧安装地址。

## Claude PTY 桥接候选

[claude-code-cli-acp](https://github.com/moabualruz/claude-code-cli-acp) 的文档明确依赖交互式 claude、PTY 与 transcript JSONL。兼容页的研究快照仍是 2026-05-14 / Claude 2.1.141，而 PendingCrew 10 月历史测试使用 2.1.291；文档的旧快照不能证明当前适配通过。它的兼容页列出 new/load/list/close、prompt/cancel、权限、工具更新等能力，同时明确不宣告 fork/resume、usage/cost 与完整 image parity。load 与无历史重放 resume 应分别协商。

[claude-agent-tui](https://github.com/zeo-workspace/claude-agent-tui) 同样声明运行官方 TUI/PTTY 并把 JSONL 转成 ACP。它是较新的桥接候选，本次只读其说明，不能据此宣称进程收尾与人工输入保护对等。两者都须固定版本做源码与离线契约检查，再按授权运行真 CLI。基于 SDK 执行的 claude-agent-acp 已从当前优先候选移除。

## 已确认的机制与改进风险

| 项 | 源码或设计依据 | token 风险 | 不牺牲质量的方向 |
|---|---|---|---|
| 旧版机长反复收到全组织概览 | Swift `Sources/Mcp/HookEmitter.swift::render` 有新可见消息时调用 renderOrgTree，包含每组最后一句 | 本组新消息可重新夹带未变化或无关组信息；组织越大越重。没有未读时并非每工具调用都注入 | 初次/恢复保留必要概览，后续按版本发送组织变化；最近消息按相关任务查询 |
| hook 与唤醒降噪不一致 | crewd `server/src/agent/inject.rs::folds` 折叠无关 ack/progress；`collab/src/render.rs::blocks_for` 逐条拷 m.text，build_batch 用该路径 | 同样的低信息消息在 hook 里是摘要，在唤醒正文里仍是全文，并可能挤占预算 | 共享降噪分类；保留人的输入、指向自己/回复自己的消息和真正结论；摘要有 seq 范围与可读回引用 |
| 接收者与上下文的放大 | `wake/src/plan.rs::plan` 中 DM 默认叫另一方，listeners 与显式目标取并集 | 收到/进度回复可触发额外 turn；多个收听者会各自处理一份上下文 | 显示源消息/叫醒理由/实际扇出；收听按主题、来源和期限限定。静默 ack 策略先验证，不改已有默认语义 |
| 共事礼貌引起往返 | world-model 要被 @ 先回一行，DM 与 reply 的规则会叫醒另一方 | 一句“收到”可能变成独立回复 turn；接收端若再确认就形成有限但有成本的链 | 先回保留成同一工作 turn 的状态/消息；无动作回执不再当作新的待办，按规则明确是否需要唤醒 |
| 任务上下文重复渠道 | spec §4.3 写每次唤醒正文 + 每轮工作项注入；Codex my_work_items 不变不重发是目标 | 实现时可能同一任务快照在 prompt 和 additionalContext 各出现一次 | 一个 task/version 在一次 turn 只有一个内容位置；恢复/压缩边界重建，其他 turn 发变化及必要当前摘要 |
| 消息长度预算单位不同 | hook 上限 9000 字符；wake 消息上限 9000 字节。完整启动 world含完整 brief，live facts 单独有裁剪 | 不能把两个“9000”当同一 token 预算；单批长度上限也不限制跨批累计上下文与回合数 | 记录实际字节/字符/usage，有可靠 tokenizer 才加 token 估计；保持人工任务全文可访问，清晰选择必要正文 |
| 连续最旧窗口可能分散任务 | render build_batch + fit_budget 保留连续最旧前缀，剩余行 deferred | 为读完历史可能产生多批投递，真正触发当前任务的消息被推后；需实测是否增加 turn | 测量每消息/任务批次数；旧背景可带有可追溯摘要，关键新指令保留完整。不能提前把未读原文当已处理 |
| Codex 压缩门槛成本 | `exec/src/codex/proto.rs::should_compact` 为 70%，昂贵回合时 45%，至少隔两回合；turn tracker识别原生 compaction | 自定义策略可能早于上游必要时机，增加压缩和丢信息后的重新读取；未证明实际双压缩 | 对照上游、记录理由/次数/usage和恢复质量，优先一个清晰策略；不能无数据直接删除已有保护 |
| Jev 每成员判断与巡检 | spec §7 每成员 noul、§8 每10消息/30min巡检；实际 hooks 默认 NoArbiter/NoIdleHook/NoExtras | 后续接线若每条进度都触发或对全员逐次询问，会放大外发；这是尚未实施风险 | 显式 @/DM/已有负责人走代码；仅歧义候选才判断，按有效状态版本复用，巡检无变化不外发；定时CPU检查不等于模型调用 |
| 启动并发与无产出环路 | 默认 max_live 可为空；breaker 10min窗口、边/目标阈值8，另有因果链；WorkConfig停下再叫预算6 | 同一错误在被拦前仍可能花若干回合；没有全局成本视图难看到多任务放大 | 保留现有人类取消、来源/因果熔断和不设总量限流决定，补原因/usage/扇出视图；按无有效状态变化早合并，避免砍掉有价值工作 |

上述是机制与风险，不是已经测得的浪费账单。特别是当前 NoArbiter/NoIdleHook/NoExtras 不会因为30秒 heartbeat自行调用 Jev 或起行政模型；不得把设计中的 future hooks 当运行事实。

Codex 当前 `session.rs::start_next` 会把本次 context传入 turn_start_params，未看到执行层对相同值的本地去重。spec提到 app-server按 key去重；本次没有验证上游是否在恢复/压缩后保留该语义，因此只记录为协议契约待核查，不能宣称已证实重复计费。会话历史仍会参与后续推理，正文没重复注入也不等于后续上下文读取为零。

## 已有保护不能算成缺失

共享游标预留/生命周期防重复、来源与因果链熔断、同目标批次合并、消息页与上限、重复发言拒绝、ack/progress摘要、系统状态变化通知、默认惰性调度 hooks 及停止恢复语义已有实现或接受记录。每次换 ACP 桥都须证明这些契约仍成立；不把审计变成重开所有已接受单元的广审查。

## 验证与改进顺序

首版先补上下文与唤醒的计量字段：task/version、来源消息/因果链、batch、内容位置、bytes/chars、prompt/恢复/压缩边界、原始 usage、可观测处理结果。量测缺失显示 unknown，不从截断字符数猜账单。

先验证 wake/hook 降噪一致、工作项单一注入位置、组织概览按变化和 DM/listen 扇出。测试保留人工指令/实际阻断全文、摘要展开、语义顺序、恢复后关键上下文重建、shared cursor恰好声明范围、停止后不复活，以及完成任务的质量。再接 Jev、巡检和大规模自动化；协议桥不增加模型回合或为了结构化输出额外叫模型总结终端。

开发流程的重复全文读日志/账本也算工程成本。机械完整校验与异常索引应由稳定工具负责，审查者仍检查工具可信度、关键失败与恢复和真实行为覆盖；修后按影响面检查，原始证据保存。审批拒绝与独立验收要求不因省 token 被绕过。每项改进以完整接受任务的成本、漏检/返工与时间对比，不能用更少 token 单独证明质量提升。
