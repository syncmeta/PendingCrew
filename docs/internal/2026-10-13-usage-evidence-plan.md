# Codex usage 纵向交付定位（2026-10-13，只读）

判断：C4 的 provider 原始成本证据尚未接通。现有读数用于自动压缩，不是耐久 usage 账本，也没有足够字段支持可靠质量/成本反馈。没有发现当前已经按重复快照计费；不能把未来风险写成当前费用错误。

## 已有链路

1. app-server 通知 `thread/tokenUsage/updated` 进入 Codex Core 的 `on_notification`，先过滤其他 thread，再进 TurnTracker。
2. `exec/src/codex/proto.rs:398-435` 只解析 turn ID、last.totalTokens、total.totalTokens、modelContextWindow；窗口缺失映射为0仅供压缩策略禁用。
3. `exec/src/codex/turns.rs:201-218` 将最新读数覆盖在内存 usage，比较累计基线供 compaction；未累加所有重复通知。
4. `exec/src/session.rs:177-225` 没有 usage 事件；Codex `session.rs:966-1055` 不向上转发 usage。orchestra core 收 Item/TurnEnded/Profile，但这些都不携带 usage。
5. store schema/sessions 无 usage 字段/表；故模型读数无法从现有 crewd 查询读回。原来真实验收 acceptance.json 是外部人工证据样本，不是产品持久化实现。

## 两个实际缺口与一个防错要求

- 原始信息在驱动层丢失：inputTokens、cachedInputTokens、cacheWriteInputTokens、outputTokens、reasoningOutputTokens，以及未来未知字段不向上游保留。缓存命中仍属于输入口径、推理输出仍可能属于输出口径，不能把各字段直接相加。
- 缺耐久留存与可读回：通知未进入 SessionEvent→Core→DbPort，因此断线/重启后无法比较任务总成本或查询采样来源。C4 的实现/审查/验证/协调/返工分类也未从这条链得到采样事实；先补来源数据，不凭模型叙述生成分类。
- 防错要求：tokenUsage.total 是 thread 累计快照，last 是最近一次读数；重复、迟到、resume、压缩前后通知不能全部求和成消费。当前 code 未做这种求和；新增单元须明确只存快照与来源身份，完全重复快照幂等，缺失字段与0分开，provider未知字段保留，不发明费用换算。

## 可独立实现单元：原始 usage 快照耐久记录和分页读回

从真实Codex通知转发只含原始tokenUsage及thread/turn身份的Usage事件；不记录整个敏感wire消息、不触发模型。归属现有session并加provider标记，经单写者持久化；保留不同total快照，不以turn ID唯一覆盖（同一turn会多次更新），对相同provider/thread/turn/raw快照去重。API按session分页，返回原始读数、采样时间与稳定记录ID。客户端暂不展示费用或启动周期性采样；不改压缩策略。

验收：纯Core注入通知覆盖真实生产解析；同一通知重复不增记录，distinct累计快照可读回；所有known字段与一个future字段保持原样，缺失cacheWrite不同于显式0；别的thread拒绝；独立数据库关闭重开后读回；真实Unix socket使用合成数据查询分页。所有offline，无真实模型唤醒。不同线程累计值不相加，没有订阅重放带来的新“费用”。

建议文件所有权（需主代理分配后才实现）：

- 新增 exec/src/codex/usage.rs（原始usage边界解析）与定向测试；修改 exec/src/codex/{mod,session}.rs 和 exec/src/session.rs 接事件。
- 新增 store/src/runner_usage.rs 与专用store测试；修改 store/src/{schema,lib}.rs 做正式迁移与导出，不在RPC入口临时建表。
- 修改 orchestra/{core,ports,db}.rs 接持久化；以实际文件名称为准，enum与fake port需一起编译适配。
- 新增 proto/src/runner_usage.rs / server/tests/runner_usage.rs；修改 proto/src/lib.rs、server/src/dispatch.rs、schema生成输出。

这些文件不与草稿/首版Swift/巡检模块直接冲突；其中proto/lib、dispatch、store/schema是跨层公共接口，必须由主代理明确单一写者或串行整合，不先散改。

本机已生成的协议证据（不新调用CLI）：`/private/tmp/pcw-context-schema-20261013/v2/ThreadTokenUsageUpdatedNotification.json`。其中last/total均含input/cached/output/reasoning/total，cacheWriteInputTokens可缺失且schema default0；原始记录应保留缺失事实，不能以schema默认推断provider实际返回了0。
