# 跨机组完整工作流实施范围（2026-10-13）

## 执行与有效决定

主代理已批准本纵向单元进入独立树 codex/global-work-flow-20261013；源基线 eaa300e4，首版冻结树保持独立。usage schema8 正在另一树推进，组织迁移版本随后串行整合；公共文件不会在同一树多作者并发修改。工作核心可在独立小 target 定向验证，全 workspace 编译由集成人协调。

有效工作模型以 spec §24.1 为准：全组织一本账，原始需求/计划/待人类处理三种条目；三种条目不等于三层目录。最多三层限制 parent 层次，依赖与来源另表达；可读代号随父项重编、旧代号仍能跳转，稳定编号不变。进展直接读取计划，视图和文件夹不复制状态。原图允许父项依赖自身子项，沿既有图方向规则校验，不将所有关系天真混合成假循环。旧超深状态须明确诊断，不能静默截断。仅有全局编号不代表这些能力完成。

前端最新确认保持原样式和sidebar；会话列表在右侧消息tab中，Todo和成员可收起。设计仍由专门聊天维护，工程不重画原型。客户端契约缺口表由独立代理核查，不替设计定案新字段。

下述为完整实施/验收范围；单元尚未交付，不以计划代替实现或迁移证据。

## 判断与完整用例

spec §4–6 + §24 的最终决定是全组织一本工作账；当前每crew WorkBook 的 W/#/H 是局部纯core编号，外层object_refs才是稳定全局引用。只去掉collab::work::local_reference的跨crew拒绝会把另一本的W1当成本crew的W1，仍没有全图校验、完成传播、真实唤醒；不能作为交付。

验收用例：人/获准负责人在机组A拆出机组B的子工作B，A的一件工作依赖并明确blockedOn(B)；B负责人领取、报告、完成；同一个提交中A卡点解除，其他卡点保留，所有依赖齐才active且nextCheckAt=now；A负责人收到一次持久化系统唤醒。跨组原始需求source的状态由全组织关联计划推导。客户端重连/后台崩溃不复用旧编号、不重新发任务。非本事项工作继续进行。暂不引入Jev重派、记忆、转交小接口或厂商内部通信。

## 权威模型选择：升级已有纯core为唯一组织账

1. 用一个 OrganizationBook/组织版WorkBook持久化现有 BookState 结构，工作项、原始需求、待人类处理各保留所属crew；来源、父项、dependsOn、blockedOn(Item/H)指向组织范围纯ID。状态/负责人/备注/证据仍只在这一份权威状态中，不在SQL跨组边表另存第二份状态。
2. 身份索引沿用object_refs。既有外部W/H/#号码与原crew+source_id alias保持不变。建议给identity记录补纯组织节点ID映射列（类型+core_id唯一），迁移时可用现有global number作为初始core ID；新对象按组织core计数器生成并经object_refs绑定，外部number继续原registry唯一发号。读写必须通过明确映射，禁止再次从旧 `source_id=work:<local>` 裸解析成组织ID。索引只有身份，无工作状态。
3. BookState增加每个对象所属crew（对象字段；不要靠session所属crew反推工作归属）。组织账本ID必须独立于管理crew生命周期；管理crew不是所有对象默认归属。工作移除后冻结/仍可读规则沿已定设计，不通过搬到管理crew规避。
4. 新状态先完全恢复/校验：owner session存在、所有跨组目标存在、parent/depends/blocked图无环、终态/来源裁定/父子约束、active检查点约束与现有纯规则一致。相互依赖循环涉及多个crew也拒绝；纯core仍不依赖SQLite/tokio/runner。

## 权限必须一起修

当前Actor role是针对一个crew的角色，collab::work::actor会把本操作crew的机长转换为Captain；全组织账若继续把Captain当全局角色将越权。

- 在纯命令中增加不可伪造的作用域上下文，由collab一次读取角色、acts_as_captain_of与对象所属crew并构造。普通session只按owner权限写；普通机长仅管其作用域，总机组机长按既定acts_as_captain_of对所有crew生效；admin保留事实处理限制，不扩业务决定权。
- 对跨组创建子项/reparent，分别检查新对象crew的创建权限以及父对象当前owner/其crew的CreateChild或Relink权限。不能因为拥有子项就随意挂到别的机长父项下；也不能仅凭parent读权限改父子关系。
- dependsOn/source是可读引用，添加/替换仍检查被修改项的写权限与目标crew未冻结/目标状态有效；不会赋予对目标项的状态写权。
- Done之后系统传播是同一已获准事务的结果，不冒充跨crew人工/机长决定。负责人回报进度沿现有规则，不替无关负责人清预算。

## 一个事务完成图与可靠通知

现有asks::with_book只加载一个crew，work::update的完成传播只比较这个book。改为with_org_book：

1. 在现有SQLite单写者事务里load唯一组织状态、检查目标revisionCAS/权限，调用纯命令。
2. 完成/人回复通过既有Draft::release逻辑扫描全组织关联项，保留其他blockers、用全组织deps_done决定active/open，记录真实BlockerCleared事件；普通dependsOn全部done的ready变化仍使用前后ready差，不制造BlockerCleared。
3. 校验整个合并图后保存一次状态，append真实事件，绑定新身份。事件索引仍可按对象所属crew读回；不能把一个源crew的事件seq强塞到另一crew已有局部序列。
4. 将受影响items按owner session合并enqueue系统wake，target为全局W；源标识由真实完成/答复事件稳定身份导出。同一个owner拥有多个crew工作仍一条wake；有限展示正文不能裁掉内部精确targets。
5. 任一身份绑定/日志/wake入队失败，整个事务回滚包括状态、revision、源对象完成和号码分配，不发生“B已done但A永远blocked”。CAS冲突不自动重写；原子失败无成功广播。
6. 沿现有render/claim guard重查target存活、owner、冻结与原因，render后重新指派/完成导致旧wake dropped；后台重启只恢复队列，不重放开发任务。Delivered仍是runner确认后记账，enqueue不等于接受/工作完成。

## 迁移必须交付，不留两套可写状态

- 数据schema版本升级，在单事务内读所有旧books与已发object_refs。按(crew, local ID/type)映射每一个items/asks/entries及parent/depends/sources/blockedOn、pendingTransfer、ownerHistory、待问关联、checkpoint/stop manifests；不同crew的W1/#1/H1互不撞号。
- 旧book中的纯seq事件、work_events、ask_meta、wake metadata、历史source alias保留原始证据。不能为了方便重新发一串“Created”历史事件。已有pending wakes不能带旧局部W访问新账；需要显式兼容旧wake读取/迁移targets，已settled receipt保持稳定来源防重复记账。
- 合并clock取已保存最大时钟，保留revision、created/updated、missedChecks及全局stop预算。`ownerHistory`顺序和完整交接说明不截断；从旧ledger修复身份缺失不允许静默制造节点。
- 组织状态写入并全局校验成功后旧books变为只读迁移证据/归档，生产所有写路径（包括ask.open/answer/default、MCP、checkpoint acknowledgement、idle/patrol/context、删除/冻结）仅操作新账，禁止双写。缺旧已发身份/损坏book/未知target整体失败。
- 合成数据库先验幂等、迁移失败回滚、备份恢复，首版独立根的真实数据切换仍按授权。不要把“只迁移全局编号”写成完成全组织账。

## 验收边界

A. 纯core：跨crew parent/dependency/blocker并图循环、完成前父子限制、source状态推导、部分解阻仍blocked、最后依赖齐active/检查点；对象级机长作用域、跨组子项双权限拒绝、管理机长沿既定权限。
B. 真实Store/DbPort：两个books都有W1/H1/#1迁移后稳定读回；不可写旧账；迁移幂等/缺权威失败/中途失败全回滚；真实完成事务生成A解除事件与owner合并wake，queue失败回滚B done。
C. 实际socket+MCP：A创建与跨组关联→B负责人更新/完成→A读回正确状态/备注/全局链接；普通B机长不能改变A工作；原来的同crew API/旧alias仍可读。
D. 实际actor+fake executors：B完成后A仅一次接受；A busy时durable pending、idle后送；重启恢复、客户端重连无重复；render后owner改变/终态旧正文不进入runner。没有真实模型调用。

不是验收：仅开放API外部ID参数、只测数字不撞、只看发布WorkChanged、只测一个book，均不能证明整个用例。

## 实施分组与冲突控制

- 模型+迁移骨架：work/{model,book,ids,invariants,perm,create-or-entry,progress,block,views,watch}，store/{schema,object_refs,work_events}与专用migration测试。可独立树实现，但不提交可运行“只开放参数”的中间产品。
- 持久化+通知：collab/{asks,work,work_context,work_checkpoints,work_idle,work_patrol}及实际相关模块；server/orchestra/{db,port,core}影响load/ack/actor接线。复用已接受通知规则，不新调度账。
- API/MCP/端到端：proto/lib与work DTO、server/dispatch、mcp registry/工具和schema；定向socket/actor/migration tests。

与正在进行usage单元重叠：store/schema.rs、store/lib.rs、proto/lib.rs、server/dispatch.rs、orchestra/core.rs/port.rs/db.rs、API schema生成输出。由主代理安排独立worktree和串行移植公共接线；不共用cargo target并发构建。work纯core与迁移测试可先独立推进，公共文件必须单一writer。usage原始事实不得因组织迁移丢掉session/thread关联；它不是此用例新增前置。

## 权威源与现状定位

spec §§4–6、§24（旧v1同crew限制被后续明确取代）；work/progress::mark_done/relink、block::release、views::deps_done/entry_view、invariants全图；collab/asks::load/with_book与work::local_reference/update/enqueue路径。备注：spec旧“done必证据”已被当前pure owner completion optional改动更新，本单元沿当前规则，不借迁移重加门禁。
