# 受管自由表：列类型、独立生命周期与动作历史

本规范合并已冻结的 v0.4 和 v0.4.1 兼容修订，定义 `managedTables.v3` 能力及 `managedTable.v3.*` 接口。交付与验收层级由[全工程计划](../plans/2026-10-13-program-delivery-milestones.md)单独跟踪；协议、源码、普通运行、完整应用与正式窗口分别验收。

六个 MCP 工具保留原名称：省略 `contractVersion` 或显式整数 2 使用原协议；显式整数 3 使用本规范。选择器一旦存在但无效即拒绝，不降级。直接 RPC 使用 v3 方法名与对应 DTO，不额外附加 MCP 选择器。公共完整契约由 `crewd schema` 生成。

## 1. 本次边界与常量

保留 crew-owned T/O、现实际作者/row policy、现通用手工边/真实链接列。表 archive/restore 推进 schemaRevision（它也见证表可写状态）；行 archive/restore 推进 row revision。归档不释放名字。精确 Decimal 本块交付；Person 沿现 managed_links。组织级创建/协作、pins 自动归档、共享视图/注入、补偿/导出不冒称完成，也不取消原需求。

- 旧 metadata 上限保留：标题256 scalars、用途1500、列名120、列总数64（含 hidden）；reason 新 V3 PatchRow/AlterTable/生命周期必填，非空、≤1500、边界无空白/控制符。旧 API 不伪造 reason。
- 新 Text：`bounds={maxUtf8Bytes:65536,maxUnicodeScalars:500}`；新 LongText：`{65536,1500}`。旧 Text 缺 bounds 解释为 `{65536,null}`，保整个合法域，包括换行、组合字符和原始 UTF8。值不 trim、不 NFC，不经 String.length 的 grapheme 数截断。
- 每格仍受65536 UTF8 bytes限制；**整行 persisted scalar cells 的实际 JSON encoded 总量仍≤65536 bytes**，沿现 collab/apply:446，不因每格合法就放行超大行。先验所有限制，超限整体 rollback，绝不截值。
- 单选 active≤12/字段，历史总数≤256/字段（含停用）；名字/值长度及整份定义预算仍独立适用，256不是保证能放256个长标签。
- **整份 canonical fields 定义 JSON 实际encoded≤65536 bytes**，涵盖 options/labels/bounds；当前旧 V2 definition 的64×120 scalars及固定结构在此域内，迁移测试必须实证极限旧定义。新增/改 schema 超限 typed invalid(definitionBytes)，不自动丢停用项或改短旧数据。title/purpose/作者等头元数据另受原上限。TablePage与RowPage实际encoded预算各256KiB，按最后实际返回ID继续，不跳过大合法表。
- History最多64条元记录且实际encoded≤32768 bytes；值块最多4096 UTF8 bytes，编码后整响应同样≤32768 bytes（JSON转义也算）。不得把 logical chars/事件数当 wire 字节预算。

## 2. 精确 scalar/option wire

`KindDraft` 由调用者提交，`FieldKind` 由 Core 回实际 bounds/options/固定 Decimal precision。调用者不能自选 legacy bounds/option ID。结构/选项名同现严格 label 校验，**scalar 文本值不套这个 trim/control 规则**。

- Decimal `Cell={type:"decimal",value:String}`。字符算法：ASCII only；`-?(0|[1-9][0-9]*)(\.[0-9]+)?`；无指数/+、前导零、小数末尾0、-0；零只有`0`。去符号/小数点后去前导0的 coefficient 长度最多38（整数末尾0仍占位）；fraction长度≤18。先字符串验证，**永不 Double/Float/BigDecimal浮点中转**，存原canonical bytes。`precision=38,maxScale=18`只读固定。`displayScale:0..18?`及`unit:≤32 scalars?`仅显示：displayScale用于最少补零，实际小数更多则完整显示，不舍入、截短或改存值。旧 Integer/Real 原样，无强制转型。
- 例：`0`、`-1`、`0.000000000000000001`、`12345678901234567890.123456789012345678`合法；`-0`,`+1`,`01`,`1.0`,`1e3`,39位coefficient或scale19拒绝。完整向量见 value-vectors.json；38位整数合法。
- Text→LongText：`maxUnicodeScalars`合并为较宽域，null代表无scalar限制，所以 legacy `{65536,null}`变LongText仍`{65536,null}`；不能变1500。Number→Text只允许 Integer/Real/Decimal；内部保存确切旧kind/真实schema事件的 readCodec witness，不改旧 cell raw tag/value。当前读投影先转成真正 Text cell，Integer/Decimal为原canonical string；Real使用服务器既有 serde_json f64 roundtrip JSON数词（golden含`-0.0`,`0.0`,`1.0`），不从locale/显示位数取文本；fresh写才存Text。Keep仍保旧raw值。其他类型转换拒绝，使用新列+显式搬值+隐藏旧列。
- 日期：`{type:"date",value:"YYYY-MM-DD"}`；公历year0001..9999，闰年400/100/4规则，无时区/时间/自然语言推断。是否：`{type:"boolean",value:false}`；false/Null/Keep不同，不默认false。
- 网址：`{type:"url",value:String}`，用现 url::Url结构验证 http(s)、无userinfo/空白/control；不访问网络；作为 scalar **保存原合法UTF8**，不按 artifact naturalRef 重写大小写/path。它不暗建artifact或边。
- SingleSelect：存ID，不复制label。**eN全表唯一**，跨field不重用；输入/投影 `OptionRef{tableId,fieldId,optionId}`三项精确绑定。存储仅在真实row/table/field的cell留eN，不另存业务标题。新表计数从1，按fields数组序再options数组序发行；AddOption取同表nextOptionNumber。每field active≤12、总≤256；全部options label按ASCII casefold唯一（含停用），原文字节保留，不NFC。停用允许旧值Keep/读；fresh Set停用值拒；Restore同eN。Tone仅neutral/good/attention/bad/info。
- Person是单值graph字段，allowHuman=true允许Human或Session；false只Session。仍用ReplaceLinks和原target resolver/ACL/wholeHiddenSet；不新增Person scalar姓名/分机副本、不新造人员B15。标量标题可选Text/LongText/数值/单选/日期/是否/URL；Person/Link不可当标题。

## 3. 名字唯一、近似用途查重与证书

精确键：`(actual ownerCrew, ascii_lower(title)原UTF8)`，不NFC、不删内部空白、不按历史名占新名字；**当前已归档表仍占其当前名字**。不同crew同名合法；scope组织级本轮不开放。旧已重复的表不合并、不删/改名，迁移成功；fresh create/rename除自身外查重，exact冲突不允许confirm绕过。

近似是固定、版本化规则 `table-definition-near.v1`，不是模型判断：ASCII lower；仅删显式集合的空白/标点 `space tab CR LF - _ / : , . ; ! ? ( ) [ ] { } ： ， 。 ！ ？ （ ） 【 】`；其余Unicode scalar含combining原样保留。取唯一相邻2-scalar grams。title近似 iff `6*intersection >= |A|+|B|`（Dice≥1/3）；purpose近似 iff `3*intersection >= |A|+|B|`（Dice≥2/3）；A/B必须非空；任一成立即候选。这样“方案对比/方案比较”会要求确认；不是声称语义相似全集。1-scalar title只精确查重，purpose仍可触发。全部计算整数，候选按title是否触发、title Dice交叉乘、purpose Dice交叉乘、T number排序；最多显示前8，`moreSimilar`诚实说明还有相近项，不输出总数。

`defineCheck`是当前源权威的**纯读有界检查**：每次最多64张当前表的title/purpose，ID keyset，包含archived；不用旧alias匹配作为当前重复，不扫行。页间用opaque签名cursor保原query/principal/scope/owner、namespaceRevision、top8候选；namespace任何建表/改title/purpose/生命周期/删除时同Tx推进一次checked revision，raw Store入口也覆盖；row写/索引维护不改该clock。Checking时无可创建证书，候选可保持空；最终Complete才回top8和`definitionCheckToken`（exactNameTaken时token=null）。证书绑定原raw title/purpose/excludingT、权限主体、scope、namespace revision和near事实；新写同Tx核当前revision，变化则重新check，不无界重扫。需要确认时必须`confirmNew=true`；不是Human审批，任何原有合法建表Actor可明确新建。

`DefineCheck.purpose` 是 `Option<String>`。旧表原用途为 None 时，允许以 `purpose:null` 和准确的 `excludingTableId` 检查该表改名；证书绑定这个真实 None，不用空字符串补造用途。新建与 SetPurpose 必须提供非空用途；旧 None 改名证书不能用于新建或写成空用途，Some("") 始终拒绝。None/Some、原查询、排除表、主体、作用域和命名空间修订均参与检查，读取旧数据不改写用途。

CreateTable必须证书；RenameTable/SetPurpose用`excludingTableId`的证书，仍先exact expectedSchemaRevision。检查可使用派生索引加速但**不能以Building/缺索引当无近似结果**；最小正确实现是上述bounded当前SQL遍历。纯读token不落业务/receipt，读取不插namespace占位行。旧同UUID回执先行；旧 V1/V2 fresh create/rename/purpose在≤64源可完整内联检查且无相近才保旧路径；需确认或更多源时 typed define_review_required，不绕过新规则、不自动续写/转V3。

## 4. 生命周期、读取预算与历史详情

表archive/restore走schema_authority；行archive/restore走row_authority（包括显式OwnerCrewWriters），两者都重验当前crew/角色/作者。table frozen阻止所有子行fresh写；恢复表不恢复已单独archive的行，恢复行不能绕过仍archive的表或crew。Archive不是W完成/业务close。重复同flag是changed=false：不替换先前归档理由/actor/time、不加revision/event/hint/progress；fresh no-op仍可记自己的原UUID收据。真变化只推进相应schema/row revision一次，Archive→Restore使旧draft过时。关联target软归档仍可被别的可写source引用，沿现Core规则。

V3 Table/Row独立lifecycle元数据缺省Active；frozenReasons准确区分Crew/Table/Row。TablePage256KiB须容单张合法完整表。Row正文只回真实scalarCells和`linkFieldIds`，**未加载不等于空cell**；Person/Link由`row.links{rowId,fieldId,expectedSchemaRevision,expectedRowRevision}`单字段取完整原LinkCell+wholeHiddenSet。RowPage包含一张table+至少一个合法scalar row的空间，超过页预算返回短页+nextAfter，不跳过行。正常link cell最多64targets；现artifact可读key回验value≤4096B+固定prefix、namespace≤512B，summary≤280scalars，按JSON最坏转义给独立机械向量`link-cell-bound.json`（约<720KiB，非按characters估算）。Row-link响应1MiB预算不是截断器；正式新建ID固定UUID/短role ID，旧producer/导入必须核实合法ID形状。尚未证明的异常大opaque ID不能被声称为旧正常合法域；保留其权威字节并typed unrepresentable，不伪造/截短，不为该假设加referenceToken协议。若正式合法入口出现反例，先给证据再冻结公开续取，不能静默跳项。Knowledge仍未advertise，不把其潜在payload混进此Core界。

History列表只含metadata/字段ID/理由preview，绝不inline整份before/after。真实serialized payload≤32KiB，物理候选≤64；reasonPreview≤280scalars、legacyReasonUnavailable明确。`history.value`用**typed path**：scalar{fieldId,side}|links{fieldId,side}|reason{}|schema{side}，没有@reason假字段ID。scalar/reason/schema用opaque cursor分块，UTF8边界安全、≤4096原UTF8 bytes且整JSON≤32KiB；schema片段是原保存JSON，不冒充现V3 schema。格式：Text/URL原UTF8（空串仍Text）；Null文本`null`；Integer/Decimal原canonical；Boolean`true/false`；Real服务器roundtrip JSON数词（`-0.0`保bits）；SingleSelect为`{"fieldId":"p2","optionId":"e1","tableId":"T10"}`的无空白UTF8-key排序JSON，不回旧label当ID。Native只按valueType拼块/展示，不猜另一种codec。

游标绑定db/restoreEpoch、caller principal、source、真实历史entry、path/side、不可变value digest和偏移；每块重新当前source读权与target ACL，权限变化即拒绝，不以旧cursor当grant。source可读不代表历史H/message target可读；受限链接整组`restrictedDetails`，不逐隐藏目标列placeholder/id/count。没有历史原值返回Unavailable，不给空串或编造旧值。旧LegacyReceipt同UUID可跨principal重复，HistoryKey使用closed codec(managedObjectsV1|managedTablesV2)+原requestId，还须opaque entryLocator消歧；它是私有来源键的读定位MAC，不是新业务ID或权限。新Event key用已有真实event UUID，不能为旧无event的动作造UUID。

历史存证应有事件头与逐字段delta child rows：列表读小header，不解析64份巨大whole-row before_json；每值详情最多触及当前字段≤64KiB旧scalar，Schema详情≤本定义上限，links逐target复核并有界续页。审计不是第二当前账，现行值仍只在managed_rows/managed_links。原旧receipt若只存result/digest，header可由实际principal/recordedAt证明，但不能把原对象author当编辑者，也不能补无据before。原V2 schema事件可用真实before/after。新V1/V2/V3真写共同audit hook保存准确delta，V2复用原event ID，旧V1真写可新记实际audit event，但exact replay/no-op不加；迁移不补假历史。

## 5. 兼容与既有MCP六工具的真实路径

现代码已核：agent/catalog.rs:717–749恰六个table工具，operation schema由`managed_v2_parameter`抽取；tools/managed_tables_v2.rs直接`parse_args<v2DTO>`；rpc.rs用`requires_managed_scope`走真实connection/token scoped lane；port.rs:1026–1140在FIFO内先后验证captured actor。因此**保工具名/角色/只读标记/幂等标记**，只扩以下互斥closed参数分支，不新增一整族工具：

| 既有名字 | 缺省/显式contractVersion=2 | 显式3 |
|---|---|---|
| managed_table_v2_get | 原{id} →原V2Table | action=table/defineCheck/history/historyValue，各自closed DTO |
| managed_tables_v2_list | 原{after?} | {after?,includeArchived} →V3TablePage |
| managed_row_v2_get | 原{id} | action=row/links，后者有field和原schema/row witness |
| managed_rows_v2_list | 原{tableId,after?} | 同+includeArchived →V3 scalar RowPage |
| managed_table_v2_submit | 原scope/crewId/requestId/operation | 同原envelope，operation选V3闭合union |
| managed_table_v2_request_get | 原lookup，原codec/原JSON | 仅V3receipt codec，不跨版本猜测 |

已有managed_object_scope_get原样发现scope，不再新增scope工具。presence-first selector：args先按现null→{}兼容规则，仅缺省version=2；显式数字2/3（整数representation）合法。null、字符串、浮点3.0、未知version、显式3缺action/非法action都拒绝，**不能serde Option把null当缺省降级**。移除selector后再用该分支closedDTO解析；catalog按同分支oneOf/const2或3生成，actual dispatch仍需校验，不把JSONSchema当授权。Tools三个新增read actions沿同只读FIFO当前Actor；submit仍原scoped lane/current admission；Admin不可借版本新获CrewMember grant。V3 cap仅整块生产Runtime就绪才发布。

新V3请求复用现managed_v2_receipts物理主键(ownerCrew,principal,UUID)，method明确v2/v3 codec。同key不同版本/intent冲突，不借版本重新执行；V1 table-family receipt在旧managed_object_receipts的相同identity也须点查防新解释（D/R其他操作不在此跨table版本规则里）。lookup按journal originVersion→原路由；缺marker按旧2，已存旧1 journal仍原1；错版本拒绝，不查不到后自动尝试写。Retired保持Historical{receipt:null}，无row才NotFound；restore旧scope只有历史，不能续执行。旧UUID exact receipt在fresh schema/lifecycle/CAS之前返回，当前读授权仍先重验。公开旧receipt原codec/bytes不改，不能用最新Row替换历史snapshot。

V1/V2 current读只投影可表达表，**过滤在keyset LIMIT前**；不能让未知kind崩整个目录，也不把新列当空。V2兼容仅Text/Integer/Real/原Link且actual cell可投影；其余LongText/Decimal/enum/date/bool/url/person用typed codec_unsupported（即使可硬转string也不冒充旧语义）。lazy numeric→Text必须先给实际Text cell且保原IEEE/Decimal；旧fresh写也执行V3真实bounds/row policy/lifecycle/definition-check规则，不能因旧enum=Text绕500限制。V1写仍保原layout/schema fence；所有权限-only引用（source/artifact/D.about/R/L/通用links）改用共同存在/授权adapter，不要求输出旧完整Table/Row codec。

## 6. bytes/digest、迁移和分工

V3 digest根node精确为{method,scope,crewId,requestId,operation}，前缀为 UTF8 `managed-v3-intent.v1` 后接一个 NUL 字节；沿原tag00/01/02/10/11/12/20/21、u64大端长度、UTF8排序键、原UTF8 strings、metadata unsigned、Real IEEE64。nil/缺省先按typedDTO统一，数组保持原提交顺序；Decimal/Date/OptionIDs始终string，不走number encoder。每个receipt核原scope/crew/UUID/opKind/digest/eventId/effects；真changed rev+1，no-op rev同/eventId=null。共享固定向量必须由 Rust 和 Swift 的生产编码器分别核对实际字节与 SHA；仅生成向量不算通过。

Core 格式从31迁移到32。保护原31备份，同Tx只新增元数据DDL，不扫消息/git、不造T/O或工作/群/模型事实。元数据保存在`managed_table_v3_meta(number PK/FK, next_option_number,archive三元组)`与`managed_row_v3_meta(number PK/FK,archive三元组)`；旧missing row=Active，nextOption从1，若schema含enum却无counter则corrupt不能重发e1。**不在旧managed_table_attributes后随意加列破坏INSERT VALUES的8列旧入口**。旧fields_json缺bounds按legacy解释，旧cells保原bytes；只有真实schema演进才保存新版显式bounds/readCodec witness。field p高水位机制不改。option专用remember/restore floor同原field规则：真正成功发行同Tx记全局非业务高水位；restore既存T next>=carry；恢复31旧T先carry后启用；新T仍e1。active12/总256不是issued数字上界，restore跳号合法。

当前源唯一性不变：fields_json/attributes是定义，managed_rows.values_json是scalar，managed_links是Person/Link；new audit仅不可变动作证据。managed_objects V1、managed_tables_v2桥、managed_links/projection、managed_message_sources、R/L/D引用校验、organization_search当前/历史slots/option labels/lifecycle状态都接同一logical schema/value adapter。表rename/purpose/option旧标签只给T aliases，不复制O；hidden scalar全slot保留、当前active过滤，restore可召回。table archive/search status用bounded row派生fanout→Building→publish，不给万行业务row批量加rev。


实施验收必须含：38/18Decimal字符边界及无Float；legacy64KiB与整行64KiB双层；definition极值bytes且每field256不是无限labels；eN table/field/counter/MAX/restore不复用；false/null/Keep；停用选项Keep与fresh拒；表/行生命周期权限/ABA/原UUID；V1/V2无旁路/原receipt；history32KiB页、64KiB完整scalar/完整reason/schema续块、权限中途撤回；6工具旧参数/默认2/显式3/null降级拒绝；正常Core/Native/MCP/paired完整纵向流程，不把DTO/goldens称已实现。

## 8. 工具声明的完整读取

每份表格工具声明仍限制在 32768 UTF8 bytes，保持闭合对象、必需键、联合分支和所有值类型。相同结构且必填判别值互异的变体只合并判别值枚举；重复参数说明只去除重复副本，完整参数说明保留。声明不引用外部 schema。

Codex 自身可信 `crew` 服务的 `tool_input_schema_max_bytes` 固定为 65536，为官方生成器的中间工作量提供有限空间；其它 MCP 服务不受此配置影响，实际表格声明仍不超过上述 32768 bytes。该值与历史接口的 32 KiB 响应上限、整行与定义的 64 KiB 数据限制分别约束，不能混用。实现验收检查完整的每个分支，不将“整体不等于 unknown”当作类型完整；所有预期字段和操作必须存在。
