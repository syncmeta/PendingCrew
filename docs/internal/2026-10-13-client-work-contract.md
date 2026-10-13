# 前端需求 → crewd 真实契约核对

2026-10-13，只读候选工作树 HEAD `eaa300e4`。源码根 `/Users/hey/.codex/worktrees/decision-backends/PendingCrew`；设计依据为共享 `design/prototypes/2026-10-13-original-chat-pane/WORK-MODEL.md`，其“下一版建议”不当成用户新裁定。当前画板工作模型尚未修正。没有编译、启动应用/模型、修改设计或产品源码。

一本全组织账、三种条目是有效目标；按角色分成“Agent 的 / 人类的”两本账不可作为接线目标。以下“已有”仅指实际 dispatch/collab 实现，不等于已接入 GUI，也不把预留 ObjectKind 当成已完成能力。

| 前端有效需求 | 已有真实读写 API / 位置 | 缺口与必须保留的错误语义 |
|---|---|---|
| 1. 一本账：原始需求、计划、待人类三种条目互相关联 | 全组织计划 `work.list/get`；全组织待人 `ask.list`；内部 BookState 有 entries/items/asks，统一 object_refs。`proto/lib.rs:69-78`、`collab/work.rs:307-312`、`collab/asks.rs:532`。 | 没有三类型统一查询，也没有原始需求的公开 create/get/list；不能把 `work.create` 叫“原始需求创建”，或把每个会话的问复制成新 Todo。`WorkPlan.sources` 能给全局 #ID，却缺原文读取入口。 |
| 2. 计划摘要/详情及负责人操作 | `work.list {crewId?,owner?,liveOnly?,includeRemoved?,after?,limit?}` → `{items,nextAfter}`；`work.get {id}`；`work.create/update/assign`、`work.history`。`proto/work.rs:6-78,127-161,186-220`；`server/dispatch.rs:427-461`。 | 列表只返回摘要，详情按需 get。写入必须 `expectedRevision` 字符串；冲突不当成功、不自动重复写；history 当前是负责人变更历史，不是完整工作事件流水。 |
| 3. 稳定全组织 ID、可读代号、旧 alias 可定位 | Plan 的 WID、sources 的 #ID、blockedOn 的 H/WID 已由全局 object_refs 投影；`store/object_refs.rs:48-82`、`collab/work.rs:84-117`。 | 公开 ask 仍是 **crewId + 本机组 H号**，ask.item/HumanWait.items 仍是机组内 W号（`proto/model.rs:479-515`、`collab/asks.rs:260-273,549`），与 Plan 的全局 H/W 不能按同字面串相认。尚无公开全类型 resolver、可读代号/改父重编号/旧代号跳转 API；不能自己造映射或只用 H号做全组织 UI key。 |
| 4. 最多三层，三类型不等于强制三层串联 | Plan `parent` 可在 create 指定；内部图对 parent/依赖/卡点并图找环；`work/book.rs:385-412`、`work/graph.rs:1-29`。 | 当前写入与整份不变量没有“三层上限”校验，也没有 public reparent。客户端不能仅截显示前三层来假装后端遵守规则；跨机组 parent/source/dependency 当前会拒绝 `work.link_other_crew`。 |
| 5. 来源、父项、依赖、卡点和分类分别表达 | `WorkPlan.parent/dependsOn/sources/blockedOn` 均有字段；create 能写前三者，`collab/work.rs:445-466`；MCP 工作工具与同一 collab 事务共用。 | `work.update` 不含改链接、卡住/拆分、放弃、分类字段，不能借 note 或 parent 伪造这些语义。当前没有跨组织链接写入契约。 |
| 6. 全组织一个待人类收件箱，带来源上下文 | `ask.list {}` 不带 home 过滤，跨所有 work_books 返回 HumanWait；可选 `homeCrewId` 过滤“home”，并非 crewId。有 question/askedBy/item/resumeNote/default/等待等级。`proto/params.rs:199`、`collab/asks.rs:532-564`。 | 没有分页/limit，不能从当前选择机组推成唯一收件箱。公开 model 只有一个 default 文本/时限，没有通用推荐选项数组；Plan 与 Ask ID 的差异见第3行。 |
| 7. 待人事项回答、稍后、不要了、结束自动撤回 | `ask.answer {crewId,ask,text}`；`ask.withdraw {crewId,ask,reason}`；`ask.changed` 推送；waits 仅列未结束事项，归档会撤回相关 open ask。`server/dispatch.rs:473-480`、`collab/asks.rs:417-478`。 | 没有稍后/标记暂缓的写 API；UI 暂时收起不等于后端 snooze。未证明“任何业务结束均自动撤回”完整生命周期。过期/已关闭 ask 拒绝（如 `ask.not_open`），拒绝时仍保留用户答复草稿；撤回 reason 不能为空。 |
| 8. 每项一个覆盖式备注 | Plan note 来源 `lastReport.text`，`work.update` 同事务回报并覆盖；`collab/work.rs:529-567`。 | 只有计划有公开 note，原始需求和待人事项尚无同语义备注写入。非完成更新要求非空 note，不能以空串清空；`work.bad_update` 必须展示，不能默默忽略。历史日志不是第二个当前备注。 |
| 9. 负责人标完成，可选凭据，无额外验收门槛；冻结只读 | `work.update {status:"done",expectedRevision,evidence?}`；WorkPlan/Summary 有 frozen。`work/progress.rs:72`、`collab/work.rs:515-548`。 | 不强制附证据/人类复审。不属于自己的代理仍受权限约束；已移除机组可读、写拒绝。服务端有 `work.revision_conflict/current_work` 等结构化事实，但 `CrewdClient.swift` 目前只取 error.message，客户端接线需先保留 code/data，否则会丢冲突上下文。 |
| 10. 进展只是计划视图，群/session/机组引用同份对象 | `work.list/get` 可按 crew/owner 读同一 Plan，`work.changed` 携全局 id/crewId；无独立 Progress 状态。`proto/work.rs:33-78,163`。 | 没有持久 ViewDefinition/view ID/列过滤分组排序/引用配置 API。客户端可暂作查询投影，但不能复制 Plan 改状态；parent 是层级，不是依赖；不要按已完成子项比例制造精确进度。 |
| 11. 独立分类、自建带类型表、对象引用和视图 | object_refs 预留 CustomRow/Table；公开 proto/dispatch 没有对应表、类型字段、分类或 view 方法。 | 这是实质未实现，不能把预留枚举、自由 JSON、Markdown 文档宣称“任意类型表”已可用。改文件夹不能隐式改负责人、依赖或状态；字段与链接规则需后端先定义。 |
| 12. 原 sidebar/样式；消息 tab 内会话列表；Todo/成员折叠；稳定三emoji群头像 | `crew.list/get` 有 parentIds/childIds；`channel.list/create`、`session.list/get`、`message.list/post` 真实存在；`proto/model.rs:127-160,416-469`。折叠是客户端状态，无需新后台账。 | 这些 API 不等于首版 View 已满足布局。crew/channel 公共契约没有三emoji字段，也没有已确认的稳定生成/持久化契约；不要擅自用 title 每次随机头像。PendingBot 的生成规则需专门对齐，不在本审计重设计。 |

## 一个可独立实施的客户端单元（仅提案，尚未开工）

新增离线 Foundation 投影模块与专用 Swift fixture：消费现有 WorkPage/WorkPlan/HumanWait，用全局 WID 保持 Plan 唯一实例，机组/owner 视图只持有 ID；完整保留字符串 revision/分页 cursor、frozen、未知 status，以及 ask 的 crew+localH 复合键。对尚无 resolver 的 ask.item/blockedOn 不做猜测性关联，并输出“待全局身份契约补齐”的明确结果。测试跨crew同H不串、长数字字符串不失真、摘要/详情引用一致、冻结和分页语义。

建议文件所有权：只新增 `Sources/Mac/Crewd/CrewdWorkProjection.swift`、`scripts/dev/test-crewd-work-projection.sh`、`scripts/dev/fixtures/crewd-work-projection.swift`，不改 UI/设计文档、Rust usage 作者文件、现有 Client/Workspace 或构建入口。本单元是给后续接线的纯离线契约基础，不宣称客户端已接 work API；最终公开三类型/全局 Ask ID 契约一旦变更须更新测试。需要父代理划范围后才实施。
