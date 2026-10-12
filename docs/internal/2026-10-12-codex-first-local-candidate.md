# Codex 本机首版候选与通信边界

2026-10-12：先让个人用户能在新版 PendingCrew 与 Codex 共事，再用新版开发记忆、知识、完整工作视图和 Jev。本文记录隔离候选；它不是安装、迁移或正式发布记录。

## 候选范围

- 工作树：`codex/codex-orchestra-20261012`，继承先前接线候选 `36f2ac81`；其他整合树的未提交维护改动没有被复制。共享 main 的降噪与方案提交独立保留。
- `serve --codex <path>` 登记 Codex Executor，沿用原生 app-server、持久 thread、世界观、机组 MCP、上下文、因果投递、停止/恢复和进程登记。未显式配置 Codex 的后台继续拒绝 Codex 启动，测试不会靠发现用户 CLI 来过门禁。
- 原始 Item/TurnEnded/Profile/Notice 进入现有 Core/DbPort；保留 turn/item ID；开始中的工具条目可见，完成记录只收 `item/completed`。回合结束复用已有团队提问/去重规则，不复制第二套规则。
- 原生人工请求保留 wire ID、thread/turn、问题和原始选项；答复绑定当前不透明指纹，重新启动生成新指纹。权限回复只能选择原请求范围；取消、resolved、回合结束、停止与退出使旧回复失效。正常请求不被执行器抢先拒绝或批准。
- `session.answer` 处理选项；兼容新增 `session.respondRunnerRequest` 处理原始问题 ID 的结构化答复，`session.inspect.runnerRequest` 提供表单，`session.interrupt` 是客户端原生打断入口。顺序写屏障确认写入后才返回成功；它不冒充 Codex 处理完答复的回执。
- 首版支持命令/文件/权限请求、原生工具提问、第三方 MCP 调用确认及 URL elicitation。尚未支持的通用 MCP 表单继续明确拒绝并给出可见通知；不自动提交空对象，不无限悬挂未知请求。crew 自己的通讯 MCP 保留既有受限自动通行契约。
- 默认招募成员按显式机组默认、当前机长 runner、历史 Claude 默认解析，读取在一次数据库调用里完成；显式 runner 保持优先。避免新建 Codex 机组在 MCP 招募时意外切回 Claude。

## Mac 操作路径

`Sources/Mac/Crewd` 是 crewd 的原生 NDJSON 客户端和首版操作窗口，不复用旧 Swift 后台的长度分帧协议。机组/session/群导航、消息、原生工具进展、初始任务、机组 ask 和 runner 人工答复、停止/打断/恢复、版本提示、断线与重新同步都经 crewd API；界面只保存临时显示状态与未发草稿，不建立第二本权威账。

- 完整 app 可用 `--crewd-ui` 选择新入口，必须在旧数据 store 与 Swift 编排启动之前分流。
- `scripts/dev/build-codex-first-macos.sh` 构建独立、ad-hoc 签名的本机候选，嵌入对应 crewd。只构建；不安装、启动、导入、更新 appcast 或发布。
- 候选默认使用 `~/Library/Application Support/PendingCrewNext`。显式新根也不能等于或位于旧 `PENDINGCREW_DATA_DIR` 内。启动界面显示独立数据空间；没有运行旧库导入。
- 首次启动先连接已有 crewd；连接不上才启动配套后台，显式配置 Codex 可执行文件。后台持有数据锁，窗口关闭不退出后台。重新连接只订阅和读快照，不重放消息、审批或任务。
- 用户日常实例、账户、旧数据迁移及一次真实自我开发任务仍需要分别验收。旧应用和旧数据继续保留。

## 验证与原始日志

最终后端门禁通过 2333 项（8 个活测试未运行）；完整 Mac 复查执行 3228 项、跳过 12 项、零失败。完整 Mac 与 iOS Simulator 构建通过，原生客户端合成验收通过。首次 Mac 失败与复查均保留；不能从这些结果推断真实 Codex 账户下的操作、安装或日常数据迁移已验收。

- C1 完整门禁：2324 通过、8 个活测试未运行，`/private/tmp/pcw-codex-c1-final-check.log`。
- 人工请求：先红后绿，`pcw-codex-human-{red,green,regression,process,serve}.log`；真实 serve/Core/DbPort 和真实 MCP helper，runner 为假 app-server。
- 默认招募 runner：`pcw-codex-default-runner-{red,green}.log`，旧默认 Claude 的行为在回归中失败，实际 Codex 机长与显式覆盖通过。
- 后端整合门禁：2331 通过、8 个活测试未运行，`/private/tmp/pcw-codex-first-complete-check.log`；包含新增原生契约/工具进展的最终 2333 项完整检查另存 `pcw-codex-first-delivery-check.log`。
- 本机 CLI 0.162.1 的 `generate-json-schema --experimental`：`pcw-codex-01621-experimental-generation.log`。出站方法/字段检查与对应精简生成快照：`exec/tests/codex_schema_shape.rs`、`exec/tests/fixtures/codex-0.162.1-client-shape.json`，`pcw-codex-native-schema-shape.log`。实验接口必须与 `initialize.experimentalApi` 一起核查；标准 schema 会省略这些接口，不能拿它的缺项判定驱动发送了不存在的字段。
- 原生 Swift/Unix NDJSON → 真实 crewd → 假 runner：`scripts/dev/test-codex-first-client.sh`；验证新建机组选中、会话选中、旧数据根保护、事件、审批、自由输入、旧答复拒绝及重连。结果与完整合成目录见 `pcw-codex-swift-workspace-final-smoke.log`，无账户/付费模型。
- 完整 Mac 构建与 iOS Simulator 构建通过：`pcw-codex-first-xcode-build.log`、`pcw-codex-first-ios-build.log`。Mac 初次全量测试执行 3228 项、跳过 12 项，两个测试共有三个断言失败；涉及旧跨进程 claim harness 就绪和长消息滚动探针。保持原始失败 `pcw-codex-first-xcode-test.log`；两项定向复查通过 `pcw-codex-first-focused-failures.log`。不能删除首次失败或据此推断全量通过；最终复查见 `pcw-codex-first-xcode-final-test.log`。

上述短文件名均位于 `/private/tmp/`。不运行 `--ignored`，不把假 runner 或纯 schema 检查写成真实模型验收。

## Codex session 的原生通信

按本机 0.162.1 schema 与 [官方子 agent 文档](https://developers.openai.com/codex/multi-agent)、[app-server 文档](https://developers.openai.com/codex/app-server)核查：

1. Codex 自带多 agent/子 agent 协作。协议的 `CollabAgentTool` 包含 spawnAgent、sendInput、sendMessage、followupTask、wait、interruptAgent、listAgents 等，`collabAgentToolCall` 带 senderThreadId/receiverThreadIds。它是明确可借鉴的原生任务内协作能力；当前 PendingCrew 还没有把这些子 thread 登记成独立机组成员，也未做其权限请求、usage、停止/恢复的完整映射。
2. Codex 桌面 app 的聊天之间另有 `create_thread`/`send_message_to_thread` 等产品 MCP 工具；它们依赖桌面 app 的任务身份与人类授权，不能推断裸 CLI/app-server 的独立进程可以直接使用。
3. app-server 提供 `turn/start`、带 expectedTurnId 的 `turn/steer`、实验上下文与 `thread/inject_items` 等底层入口。没有从此次核查发现稳定公开的“任意独立进程/session 自动互发消息”总线。`thread/inject_items` 是向模型历史追加原始项，不是有可靠回执的团队消息服务，更不能把队友的话伪装成 assistant 历史。

首版保持：长期、独立的团队成员由 crewd 的既有消息账、身份、提问、投递和恢复契约管理，底层经 Codex 原生接口投递。原生子 agent 作为后续临时任务内协作复用路线：先核查同一代理树的作用域、MCP 身份继承、外部控制、人工请求、原始 usage 与父进程退出，再决定映射范围。不因为“统一”为目标重新实现 Codex 已有的 spawn/message/wait。

协议转发本身不需要另一个模型中转；模型读取消息、回复和各子 agent 的工作仍消耗 token。原生子 agent 不保证总成本更低；完整 fork 的历史、重复工具输出和多层协调仍要计入实验。记忆、知识、Jev 不因此成为首版前置。

## 本轮成本与质量经验

- 先做短的纵向验收：新建空机组 → 真实服务端 → helper/tool → runner → 客户端。执行器自身测试通过没有揭示默认招募仍选 Claude，也不能代替实际客户端的导航与答复。
- 同一功能先核查本机 runner 生成的实际契约和能力开关，生成精简字段快照；把版本漂移交给机器检查，减少凭印象重读和重写适配器。
- 分开记录入队、确认写入、runner 接受和任务完成。错误的“成功”会带来重复派活和后续返工，节省这类 token 需要提高证据精度。
- 缺项/失败先用定向回归定位，保留完整原始日志；新源变更或实际失败才重复完整门禁。工具日志保存在文件里，日常上下文只带结果、异常与路径。
- 限定首版交付范围，沿用原有 actor/数据库/提问规则；知识插件与全量工作模型放进方案，避免同时维护临时权威账与最终权威账。

这些是本轮观察到的机制与做法，不是已经完成的 token 节省率评测。接受后续开发单元时仍记录总成本、缺陷、返工与耗时，不能用模型调用减少就推断质量更好。

## 合成窗口操作补充

独立候选在专用 `/private/tmp/pcw-codex-first-gui` 中，以假 Codex wrapper、独立 HOME 和合成 Git 仓库实际打开；新建机组、选择目录、启动、原生命令确认、自由输入答复、假 agent 存活时重连和停止已从窗口操作验证。没有调用真实账户。测试窗口及其自有后台已正常收尾，保留合成日志。

窗口发现 `channel.list` 元素是 `ChannelSummary { channel, … }`，客户端先前按裸 Channel 读导致群选择为空；修复显示投影，并新增从原生 Mac 状态模型到选群、发消息、唤醒、第二轮原生答复、重连和后台休眠的完整合成回归。旧代码在群可选断言处失败；最终通过记录在 `/private/tmp/pcw-codex-channel-message-final-integration.log`。首次错误使用底层 client 单独重连的测试保留在 `pcw-codex-channel-message-integration.log`；最终测试走正常 workspace 重连，管理同步任务与连接代际。

原生请求的完整命令/目录使用固定纵向尺寸显示；启动中的会话不再被暂时标成“已停止”。此补充是合成窗口与客户端验收，真实账户的首个任务和日常迁移仍另验。

进一步经验：契约检查必须核查嵌套结构和实际可操作状态，不能只确认 JSON 字段名存在或客户端能连上。先验收一条纵向工作流能以很小的修复避免用户接手后才发现基本沟通失效。

最终 `bec60f86` 构建也从实际窗口完成恢复已有合成会话、在“与我沟通”群发送消息、唤醒假 runner、命令审批与自由输入答复，再停止并退出。窗口及其自有后台已收尾。候选产物为 `/private/tmp/pendingcrew-codex-first/PendingCrew Codex First.app`，`source-head.txt` 记录完整源码 SHA；源码分支已推送。这仍是合成 runner 验收，未使用真实 Codex 登录或额度。
