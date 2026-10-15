# Jev 首轮完整模块：实施与证据边界

2026-10-15 用户已批准 TypeSafe 单一手选 provider、Choice/Score/Predicate 统一判断，以及需求接收路由、消息唤醒、独立消息归属三业务。未配置不外发，非法/低置信/拒绝/未知走既有机长与确定性规则；保留显式 0.5 阈值，概率说明为原生、未经新校准。生成式仅建议接口，不实现其他厂商/OAuth。侧栏不排序；个人记忆与团队知识仅方案；已接受 M5 不重开发。

当前为完整移动实现与作者 fake 验证层，尚未整体 QA 接受、正式组合或真实 provider 验证。backend 是唯一 Rust/schema/运行夹具作者；client 是唯一 Swift/设置 UI/build/workflow 作者。开发树继承的八项 Native15 增量不得整体合入；最终须相对正式 Core14 精确独立 patch，组合后的必要 fake/strict 检查另列。现有 Core14 候选、normal、包证明永远保留实际 compiledRef，不因本模块改标。

## 正式公共合同

三方法为 judgment.settings.get / judgment.settings.set / judgment.inspect。wire02 精确不可变包在 /private/tmp/pcw-Jev-first-wire-contract-02，根锁 SHA 6ea295eaf57eeec76d90ce702eb1c6aa3c4f94625444d5725e6e4f555cea9afa；仓库合同见 2026-10-15-jev-public-contract-02.md。wire01 保留历史，不供接线。UI 配置接线由 client 独立实现；RPC 存在不等于用户界面交付。

get 的可选 requestId 是纯只读查询，known 才返原 result；not_found/historical/unknown 均不能推断未执行或自动重发 set。权限是原可信本地 Human anchor 加原 local.clientTrust 完整门，不等于 direct-child 生命周期 owner；code-admitted、remote、agent 不得秘密管理。inspect 受原 crew/channel ACL，封闭脱敏，不返回 prompt、原供应商 body、secret、内部 floor 或 db 标识。

生产配置只允许 HTTPS；旧 library endpoint_for 的 loopback HTTP 兼容不是过去公共配置已支持的依据。本模块 HTTP 仅显式 cfg(test) 私有 FakeJev 注入，不增加 wire/env 开关。provider、完整规范 endpoint（含 path/query）、credentialSource 改变时 keep 拒绝；原 key 不得沿用到新目标。

## 生产接线及权威

生产 Runtime 已接源事实 hub、启动恢复、有限并发、取消与 shutdown；DbPort 避免原消息自动规划重复处理。批判断只携必要有界文本和 request-local aliases。单一 provider 缓存按完整状态版本区分，singleflight 保留已派发所有者，调用者取消不发第二次。缓存至多 256，32 个运行槽，历史扫描以 64 行 keyset 页保留有界 DB turn。批/实际请求 64KiB、响应 1MiB，总业务绝对期限 2 秒含 key load/供应商重试，后到等待者不能延长原期限。配置撤销不可逆；原始 usage 仅内部记录，公共仅闭集计数/模型/出处，未知 attempts 为 null，不伪称 0。

Requirement 正文不变，路由只将原需求交合法既有 captain/worker，不造 Plan/owner/验收/DoneLevel。需求状态来自 WorkBook.entry_view，而不是旧 todos.status；已有合法 Plan 接单使晚到 route Suppressed，保原 Work owner。消息 attribution 为独立 typed link，不改 inReplyTo、mention 或 Ask。关联总 revision（包括候选页变空）、对象状态/revision、群/成员/listen、Stop floor+独立 Stop epoch、配置/DB/恢复 epoch 在派发前与 effects 同事务重验。settle 与唯一 wake 同事务，真正交付仍由原 Core actor/typed ACK 负责；判断 Settled 不等于 runner 已消费。

## 秘密与恢复

pending barrier 及无 secret 的 Unknown 结果先持久，再调用 KeyStore，最后配置与 credential binding 成对提交。每次显式 set 使用新 slot，未知晚写不能污染后来明确配置。原 requestId 重查只读已知结果，不重写 KeyStore；Unknown barrier 留存并需新显式 Human 决策。get 仅 reported presence/unverified，不读 secret 或声明已可用。通用 RPC receipt/journal/event 不能保存 credentialUpdate.value，Debug 脱敏不是唯一防线；秘密仅驻临时参数/KeyStore effect，结果与 SQL 投影不含秘密。clear 撤当前绑定，不保证已未知且失联旧 slot 的物理删除。

同 DB 正常重开：pending/dispatched 保守机长 fallback，不重新模型调用；settled 不重投。官方 restore/import 在原同事务钩子写新 meta epoch，无 DDL、不占 15/16、不改旧恢复算法。另以既有 remote_configuration.revision 作 observed 见证：旧 Core14 正式 restore/import 原钩子也递增该 revision，见证失配即历史化；正常新 remote.configure 仅在原 epoch/observed 与 CAS 前 revision 已匹配且配置形状有效时，同事务推进 observed，不改 epoch/秘密。失配后 configure 不能洗白；新显式 Human 配置才重新建立 epoch。新判断/配置 binding/request 均比对 epoch；同 dbId 的恢复仍历史化，旧 secret 不加载，新事实未配置回原确定性路径。未知版本/损坏 failclosed，不当 absence 重新询问模型。裸文件回滚无法凭此发现，不能宣称任意备份回退 exactly-once。

## 已有作者证据与剩余验收

三业务与秘密运行验证均自有 TempDir SQLite、Fake file KeyStore、显式 loopback FakeJev 或既有 StandIn；无真实 Keychain/provider/model、SDK/App/CI。格式迁移另使用自有 false/false Core daemon，其实际范围单独列明。实际 server durable07 六条包含三业务、Stop 竞争、未知 barrier 真实关闭重开、pending 重启零请求、正式 backup/restore 同 dbId 新 epoch、原 DbPort/Core actor StandIn typed ACK 一次。它们不证明物理原生消费或外部 provider 兼容。provider03 八条包含共享绝对期限、撤销、调用者取消、响应界限；collab05 六条包含真实合法 Plan 接单但旧原行状态未变、对象关闭/取消关联/成员撤除使晚到结果失效。server durable10 八条另核旧正式恢复见证/正常 configure/失配不能洗白，原09非canonical假路径失败保留；最终 strict 与正式 Core14 组合源验证应以实际终态另行记录，不能把旧绿外推到后续输入。

原 source-only 自动审查拒绝已保留；同一精确动作按用户授权区分“实现未来配置后的代码”和“现在实际请求”复核放行（chunk 4b2dbb/exit0），不是当前阻断。旧 server01 误用广 test 图遇旧集成 feature 条件编译失败，随后限定 --lib；collab04 使用固定大厅移除成员被原权限拒绝，后用合法 Topic 完成 collab05，未放松生产权限。所有原失败保持，不改标。

整体交付仍需最终静止 source、API schema 消费者、秘密与版本/权限完整有意义控制、相对 Core14 的精确适用/组合 fake 与 strict、独立可审 commit 集和 QA。真实模型调用不在本轮授权；原前端完整配置 UI 需另按已锁 DTO 验收，不能以后台组件绿冒整产品交付。

## 正式格式兼容裁决

旧14消费者Ordinary路径实际源码不能安全承载新route wake；根已裁Jev正式格式15而Native预留16。详见2026-10-15-jev-format15-compatibility.md。此裁决取代早期“不占15/16”的设计状态；只增加原子格式标记与版本屏障，不新增表，不把任何Native WIP混入。旧14组合绿不改标15；正式迁移 source02 已实际完成，记录追加如下；15上必要 fake/strict 终态及整个模块独验仍分别记录。

## 格式15作者实际收口记录

相对准确31f0/Core14导出的无Native组合03含402 Rust输入。诊断binary SHA d8b3bcac3f224bb9b3b632d94108ccb3c4967955f8a4ebe8eea4a885292595ec，实际90832/0（chunk56952d），仅机制诊断，不是strong normal。迁移source02实际单次工具chunk baecb3/exit0；/private/tmp/pcj15-migrate-owned-02/result.json SHA 8ffd1f2bd462379847651385f730ecb32698d27f0588cd552a176706c294a78c。真实旧14 normal15建库；备份障碍拒绝原文件不变；注入marker故障后由真实old14 serve/查询/hibernate建立停态，再核版本/marker同事务回滚；清故障后保护备份升级15，无Native表；旧业务UUID/正文/receipt保留；old14拒15无socket且DB/id/sidecar的内容SHA、mode、inode、size不变；新进程15重开相同事实。全部8个精确Popen已reap，false/false，无session.start。比较没有采mtime/ctime，不声称全部metadata相等。原source01因synthetic writer留下旁文件而被严格observer拒，现场/失败保留，不清旁文件救绿。

15组合实际server judgments八条通过（1966/0，chunk630e34）；collab六条、store恢复见证二条与格式原子故障一条通过（41543/0，chunk0b5d5b）；版本保护七条通过（直接exec，chunkb3dd1c/0）。这些覆盖的是上述源码的Fake/SQLite与版本机制，不证明真实供应商、原生进程消费、Linux15 Release或原App配置界面。公共inspect reason=answered仅表示供应商返回合法答案，不表示阈值已采用；最终recipients/attribution才是经权威回验后的实际选择，低置信/生成估计仍按既有fallback。

最终格式15 strict default workspace/all-targets通过（46595/0，chunkd53974；日志strict-default-all-targets-format15.log，25.65s），fmt check无差异；wire旧enum/Box的完整JSON与JsonSchema等价控制通过（92163/0，chunk189126）。同诊断binary的schema命令只生成schema，实际与已提交schema完全同SHA6fb5c3f0a1ea07a03cce59e1974e21af5190b492b71fe4e4ecde84d2bcc78ac6。没有因此启动Core/秘密/模型。原组合02 provider八条、权限六条与wire02语义证据按相同生产字节复用；格式变更相关路径另由格式15实际控制覆盖。不声称full/all-features或双平台最终回归通过。
