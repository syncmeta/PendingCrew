# 秘书案型与给人的简报：完整单元实施合同

基线 `fa9323d0fc2378ced48d8460744d21ff6f7936a6`。这是实施合同，不是已实现或验收声明。源码、隔离运行、真实服务、原生窗口及安装发布分别验收。

## 原规划与已经存在的能力

原有效要求为 `docs/superpowers/specs/2026-10-05-crewd-backend-and-work-tracking-design.md` §8.3 的秘书行（代码计算等待和提醒、Jev 判断同类、行政 session 写给人的简报），以及 §8.4 的有事才启动临时行政 session、处理后退出。§8.2 不赋予行政业务决策权。组织模型 proposal 不是全组织读写授权，先保持同机组边界。

main 已有 `admin_human_brief::get` → `adminCase.humanBrief` → `CrewdWorkStore.humanBrief` → `CrewdHumanBriefView`：实时待人清单、未解决案件样本和保存的巡检记录。它没有模型正文，不是待重写的模块。`patrol_brief` 给机长的持久去重 wake 也继续保留。H33 同类提问整理独立保留，不合并 H、不代答、不把整理当工作完成。

剩余是明确来源及权限的秘书案型、原临时工厂执行、绑定事实版本的正文发布、过期投影与原生读取；本单元一次贯通这些环节，不靠静态摘要替代行政 session。

## 权威与案型

新增明确的 Secretary 来源，不以空 W、虚构 Stop escalation 或名字为 Captain/Admin 来取得权限。复用原行政 Case 编号、持久 decision、启动调度、`AdminLaunchPermit`、注册与 native execution generation、原认证拥有者、撤销和实际退场；不增加第二套角色、工作账或可替代的会话工厂。

原 Stop 来源的序列化、revision 和 W grant 不变。秘书来源用独立 typed provenance 绑定真实来源快照 ID、机组、原 captain/native 认证拥有者、真实 dbId/restore epoch、事实 revision。它没有 Stop budget 或 W targets；这些旧字段的空值只在该明确案型中合法，不能反过来放宽旧案型。旧客户端不认识案型时保持拒绝，不能误按 Stop 案件显示。

秘书 session 只有三个窄工具：读取绑定快照、发布给人的简报、读取原发布结果。它不得列全组织、读快照以外事项、调用一般 post、回答 H、关闭或重派 W。普通 Human/Captain 工具不变。原 Stop case session 不能调用秘书写工具，未绑定/伪造 Admin 也不能调用。

每次工具调用沿原 token、connection、admission、case/attempt、执行 generation、captain、config/Stop、Scope 与事实 revision 全重验；运行中变更撤销原代，不切换账户、执行器或来源。最终发布在同一个真实事务中重新验证，不拿提示词、角色名、先前快照或已完成结果当权限。

## 事实、触发和去重

事实取原同机组只读简报投影，保留原 H 编号、问题、默认期限、来源与等待层级，以及案件/巡检的真实身份和观察时间。原 `humanWaitsTruncated`、`caseCoverage`、`findingsTruncated` 保留并传给作者和读者；样本不能被概括成完整组织盘点。秘书自身的案型和正文不回流进生成来源，以免自引用循环。

不可变快照固定原消息/对象身份、问题/状态、截止时间、原巡检轮次和当前来源版本。revision 排除请求观察时间、每毫秒递增的 waitedMs、当前 session 的工作心跳等纯计时噪声；等待跨既有层级、原 H 答复/修改/撤回、来源/配置/Scope 改变则按真实事实处理。没有事项不建案，不启动会话；相同事实只存在一个来源和案型。一次新巡检本身不等于新事项或新生成意图。

自动观察只沿现有有界巡检/行政调度入口处理已到达的同机组，不另开全组织轮询。使用原 Jev 原生概率判断当前有界事实是否值得成简报；缺配置、低置信、生成概率、缺原认证拥有者或 Stop 都不开放执行。Prepared 可按原规则恢复，已 Dispatch/Unknown 不重投、不换 UUID 或另起会话。

## 正文发布与恢复

作者返回有界正文和快照中允许的原对象引用，不接收任意 crew、目标接收人或执行动作。正文显式属于行政 session，不能冒充 Human 答案或业务决定。原对象链接保持一项对应一项；同类建议不删原 H。

发布命令含原 request UUID、case/attempt/generation、来源 revision、正文与引用。校验正文 UTF8 上限、引用上限、闭集类型、无重复和全部来自本次快照；同事务重验后写不可变版本及原请求 receipt。相同 UUID/内容读回同一结果，UUID 内容冲突拒绝。记录 Unknown 时只读回原 receipt，不能自动重发或另建 UUID。

发布的成功只代表正文已保存。秘书案件结果只有在实际原绑定作者、来源和 receipt 核验后才作为「本简报写作事项已完成」，不会使简报中 H/W 变完成。随后原 native release/退场链仍需实际进程退出；通知和结果不冒充已退出。旧正文可作为历史读取，事实改变标 stale，不替换成当前结论；新 Scope、缺来源或损坏证据不发布新内容。

## 存储与迁移责任

新来源/正文/receipt 是持久派生证据，不是业务账权威。新增表计划包含：不可变秘书来源、source→原对象引用，以及发布正文和原 UUID receipt。真实 crew/source 外键与原硬删除保持，结构检查、正文/快照 UTF8 容量与同事务回滚必须入账。旧 Scope 只作历史，不成为新的执行权限。

公开现行 H33；私有查询候选已占34但尚未交付。秘书模块不依赖该候选，不自行更改 ACTIVE_SCHEMA、激活34或擅自占下一号。实现提供独立 install/migration 及严格验证单元；最终生产代号和与查询分支的重排由 Root 统一裁定，完整迁移/恢复/回退门之后才激活。没有迁移能力时调度不开放，不以测试专用开关替代正式安装合同。

## 原生与完整验证

原事实面板继续可用；专用正文卡显示作者、来源时间、当前/过期/结果未知状态和完整性提示，可打开原事项。载入和刷新绑定原连接/crew/generation，迟到结果不进入新环境。原生成任务状态、发布 receipt 和进程退场分列，按钮不提供代答或自动业务关闭。

必须覆盖真实来源→原 Jev→原工厂唯一注册→窄工具读取→发布→原生读取→原退场完整链，以及：无事/相同事实零重复；并发单赢家；未配置/低置信/错误概率；普通 Admin、其他案型、跨机组、错 token/attempt/generation；H 答复/撤回/到期、captain/Stop/config/Scope 改变；发布前后竞态和整笔回滚；Unknown、重启和完整恢复不重投；旧正文 stale；缺/坏快照、外来引用、超长中文、重复/冲突 UUID；真实硬删除及所有派生证据清理。单个 fake 返回或源码测试不能据此称已交付。

## 预定写域

新 Collab 秘书模块/测试及独立 Store install；原 `admin_human_brief` 事实收集的窄接口；proto 行政类型与方法清单；原 `admin_cases` / execution / outcome / session_retirement 的明确案型分支；server 行政调度、agent scope/catalog/port/三个窄工具接线；独立 HumanBrief 原生 View、WorkStore 和测试夹具。保持 Root 暂独占的 `CrewdAdminCasesView.swift` 不变。Source/Row/Work/Native/维护预算线及 G 不改。

## 本次单元与全工程剩余范围

另核主工作树私有原规划 `docs/private/2026-10-07-studies/org-os-design.md`：L462 的「每类提议采纳率按周统计、低于30%停类并在行政处简报报告」依赖该提议模块的生产事实，L730 的组织本体行政巡检明确留到 M6。本单元不伪造这些尚未提供的统计，也不能关闭该范围。此处写作只消费已有同机组 H、普通行政案与保存的巡检事实；后续真实事实生产者接妥后再按原权限纳入。

`docs/internal/2026-10-31-decision-implementation-ledger.md` 顶部与 `docs/superpowers/plans/2026-10-13-program-delivery-milestones.md` 当前条目都仍将简报列为待交付；本合同及源码不替代 Root 的集成、保护迁移、实际模型、原生窗口与安装验收。正式迁移代号、schema/version_guard 保护校验、API schema 产物统一由 Root 激活及验证。

## 原请求回执的资料最小化

`adminCase.secretaryRequest` 仅通过现有 `command.submit` 写入，原公开回执只记录 `crewId/sourceId/caseId/sourceRevision` 四项来源确认；不复制快照问题或正文。原生收到提交确认后独立只读载入专用简报。Jev「是否值得整理」的持久摘要只留行政事实、匿名占位和覆盖限制，不复制原问题文字。包含原问题/正文的新派生记录仅在带原对象关系和删除清理的秘书来源/发布表中。原执行/native 供应商自有历史的既有保存政策不由本模块改变。

正式接入需 Root 统一修改 `store/src/schema.rs` 的新代迁移与当前代校验，以及 `store/src/version_guard.rs` 的完整/知识恢复/旧代输入保护；本分支没有假占34、没有默认开启、没有修改 `ACTIVE_SCHEMA`。同时由 Root 生成并逐字核对 proto API 产物，再做真实普通 Core / Swift / 原生窗口验收。独立 migrate/strict validator 是供正式迁移调用的完整实现，不是日用数据库已升级的证据。


## frozen-03：独审阻断的完整后继合同

冻结02独审报告 `/private/tmp/pcw-admin-brief-independent-review-coewxk99/REPORT.md`（SHA256 `fcf32bf8280b3c5fe4d346bf75aed685077e13b32cf80cf35ad8e71c33dab961`）指出递归与两种瞬态失权路径。它是未运行的源码 NoGo，不能记作已执行 RED。冻结02原包保持不变，本后继继续等待 Root 执行确定性 RED/GREEN 与独审。

1. `recent_for_human_brief` 自己在原 SQL 候选、LIMIT、project_at **之前**排除全部 Secretary。下游 filter 只作额外防御。普通 Stop/HumanRevalidation 的 decode、投影、样本和 coverage 不变；不存在先投影秘书再隐藏的入口。
2. 秘书 checkpoint 的明确真实权限 tuple：原 captain/session id、crew、真实 role、executor、原 native agent identity、model/effort、desired/resume failure、工作目录/worktree；crew 的真实 id、captain 指针、工作目录、removed 状态、management/父范围；并保留原 config/Stop、managed/judgment Scope 和真实语义版本。不包含 was_working、active/output/progress 时间、updated_at、标题/侧栏状态。原 Stop 的全 Session/Crew 语义不改。秘书已发布的 receipt/Resolved 分支也用原 checkpoint 的原始事实版本核对当前真实权限；这只允许历史回执核对，不放行新写作，真实事实发布门仍须 current。
3. 秘书巡检输入只来自实际周期 producer 的最后已完成观察。原 realtime Human 面板继续显示当前 InProgress。新轮起点捕获真实 db/personal Scope，完成事务还须同一 Scope 和 judgment epoch，才保存同crew单份 `patrol.secretary.completed.v1:<crew>`（有版本、Scope、原完整 run、checksum）。查询不补证、不造空完成。旧库 Completed 或旧半轮没有该起点凭证，不能被读/完成时猜成新凭证；等待新真实轮次。坏证据拒绝，缺证据明确 NotRecorded，恢复后旧 Scope 不能复用。
4. `patrolRevision` 绑定该已完成轮的原 finding 身份与 counts/scanned coverage/truncation，排除 run UUID、时间和新轮瞬态。相同结果的新轮完成后，原来源/作者/attempt/generation/UUID不变；同计数换真实 finding 对象、H/配置/Stop/Scope 等真实变化仍严格失效。不可用“另换 UUID/复活 Unknown”绕过。正文明确给出所据完成时点和未纳入后续轮次，缺完成来源不能被理解为没有发现。

新增夹具沿原 H、原 Stop、原 periodic tick、原 prepare/dispatch/factory/nativeLease、真实活动/输出/进度 API、原 publish/receipt、实际文件重开链：14个秘书仍不挤普通样本；正常 captain 活动不换作者，真实身份/profile/Stop/config/Scope/generation改变拒绝；原新巡检开始/部分页/相同结果结束不撤 writer，之后原 Work 真实完成及新 Work 造成同计数不同 finding 时失效并新建真实来源；旧/坏/缺/跨Scope半轮不盖新完成凭证。服务器原 HTTP→Core→socket 夹具在已完成的真实巡检来源上加入正常活动控制。Swift 夹具拒进行中或缺证据版本的秘书来源。所有夹具在本分支均未执行，不能称验收通过。
