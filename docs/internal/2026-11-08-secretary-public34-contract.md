# 秘书 public34：保护迁移、恢复及 API 接线候选

本模块以已 source 接受的 frozen-03 / `466d704abe8b82f80a043918cdc7aecd824315f2` 为准确基底。Root 明确裁决公开34只用于秘书；未读/未合私有G、未改G旧证据、未把同号当兼容。本文件和源码是后继候选，作者未编译、未执行SQL/恢复/模型/窗口、未更新日用库、未实际生成API或发布。

## 原保护边界与正式接线

原 `schema::migrate_guarded` 在非空旧库迁移前 integrity-check 并创建原版本保护快照。新最终行政事务把尚未装过的H33及秘书34一起提交；从33开始则只提交34。秘书四表、module格式/原dbId绑定、public34格式/原dbId绑定和 user_version=34 是同一事务。DDL或最终marker失败不能留下半套目录、部分新身份或已升号的失败库。更早旧代转换继续使用原历史链及原快照规则，不伪称整个4→34历史都变成一个新事务。

公开号34必须同时具有原H33严格权威、秘书精确DDL/STRICT/FK/所有源目录对象、module/public格式、原dbId绑定和正确application_id。额外秘书表/索引/触发器、未知秘书格式key、缺/错marker或dbId，以及既有禁止的work_view目录/managedViews私有格式都拒绝。前33必须确实未安装秘书目录或marker，也不得留秘书完成cache、新起点Scope凭证或秘书案型旧迹；先前未激活的33秘书候选不是可自动领养的生产33输入。此处目录严格性针对行政格式，不把任意历史Core测试表当本次秘书业务表来重写。

`version_guard::preflight` 对冷源/最后提交WAL cut在私有副本上做共同资格核验，在原 chmod、恢复WAL、SHM、迁移及目标变化之前拒绝不支持格式。实际WAL切点与原源census仍按原协议重新核验；有效在线备份仍保原并发读合同，不假称其SQLite只读连接没有sidecar行为。所有新备份/恢复暂存输出在发布前也核行政格式，不只过quick_check。

同一 `schema::validate_administrative_format` 接到当前打开、保护升级、冷/WAL输入及Core恢复；`Store::recovery_input_guard` 的registry/进程恢复资格继续复用原preflight，先核Core再碰进程registry。恢复不走普通会话准入，不改用户控制权。

## 恢复与旧半轮

原Core restore仍保原H/W/消息/发布身份，沿已有remote/native/跟进/公开回执/local client/judgment/personal draft/search/notification/input失效事务更换恢复范围。原UUID保留历史结果，只查原请求；旧秘书publication保留身份/正文，不成为新Scope读写或启动权。原current/legacy输入都由公开34保护链读取；真实关闭重开不能重投原作者。知识库自己的`Knowledge::restore`只恢复独立knowledge文件及其原回执，不回退Core ACL、H/W、消息、秘书Scope。Core-only不能分裂非空knowledge或native-data完整权威；原完整native控制器未开放的路径仍明确拒绝，没有新增旁路或假“完整恢复”实现。

新增真实控制：在 genuine33 运行原周期巡检至半轮，拍真实文件后经 `Store::open` 保护升级34；原Saved字节及原Scope保持，旧半轮完成仍没有秘书完成证明，读取不补证，不自动重放旧H。原下一新轮捕获34范围并完整完成后才有可用证明。迁移不初始化observer、不把旧Completed或旧半轮猜为新凭证。

## API 唯一生成者与235基线

现有唯一生产入口是 `crewd/crates/crewd/src/main.rs::Cmd::Schema` 调同一 `crewd_proto::api_schema()`；仓库没有另一个API生成脚本。`scripts/dev/build-crewd-host-release.py` 只调用准确binary的`schema`并核验整份产物，它是资格器，不是平行类型生成器。

当前未修改的跟踪产物仍为235方法：字节SHA256 `d12fe90a6a73a2f7c84e2646cf5ca385d31d2e13cd553da7d83686d98b730810`，canonical SHA256 `c8c0de2f17870c7a931105a4d0dd9bbd3fb0eb36a26079d800282a34213f93d5`。旧Linux资格代码仍准确锁33/235及该完整hash，不能将它或旧运行改标34；它会拒绝新34源是正确行为。新Linux资格需Root基于新真实schema与新运行另行核定，作者不预填新hash。

源码只新增 `adminCase.secretaryBrief` 一个直接读RPC，进入api_schema与手列CORE_METHODS；`adminCase.secretaryRequest`只在原PublicCommand oneOf，不是raw写别名。新 `AdminSecretaryRequestResult` 通过同一generator显式进入顶层$defs，仅crew/source/case/sourceRevision四项，不藏另一份问题正文。新总数按此准确模块应236，不含G方法。旧method形状及旧DTO保持，只有PublicCommand和AdminCaseProvenance增加明确变体；旧客户端默认排除秘书案型与新capability门保持。

Root在准确合入源、唯一编译队列中执行（作者没有执行）：

1. 先将现跟踪235产物原字节保存到Root证据目录，例如 `$stage/public33-api.schema.json`，核上面的两个hash；勿先覆盖baseline。
2. 在 `crewd/` 用原入口生成到新的证据文件：`cargo run --locked --offline -q -p crewd -- schema > "$stage/public34-api.schema.json"`。保准确源/normal binary identity、完整stdout/stderr和exit code，不把旧binary输出当新源产物。
3. 回仓库根核真实输出：`python3 scripts/dev/check-secretary-public34-schema.py --baseline "$stage/public33-api.schema.json" --generated "$stage/public34-api.schema.json" --source "$PWD"`。此新脚本只读/检查完整基线、全部旧methods/DTO、仅新增合法读/typed变体/秘书defs及34源码，绝不生成Schema。保存打印的新整份hash作为新的待审证据。
4. Root核完后才把实际stdout原字节更新到 `crewd/schema/crewd-api.schema.json`，以 `cmp` 逐字核对证据文件，再在`crewd/`跑原 `cargo test --locked --offline -p crewd --test e2e committed_schema_matches_generated` 以及proto/admin秘书契约控制。旧235资格保持原来源；新236资格及平台打包另行完成。

## QA 与证据界限

本候选新增源夹具覆盖真实4/8/21/32/33保护快照→34、33数据/默认期限/大对象/消息/原UUID保持、DDL及最终marker故障回滚、current纯验证、冷/WAL坏格式及同号拒绝、Core旧/新restore、knowledge-only不改Core和完整分割拒绝、33旧半轮中途迁移、旧Scope不可再发布/再启动。所有均待Root实际执行。

Root在准确03/I输入上的实际反馈仅作为该输入的证据：Store3通过、Collab97里96通过而prose scanner指四个测试中文literal、授权Server87里82通过/3失败/2原忽略。这里按Root准确因果修正三项QA：四个数据literal换等值Unicode码点、不改scanner/运行字节；catalog原readOnly/passive精确列表补snapshot/receipt、idempotent写列表补publish，不改生产注解和destructive；原agent.call decoded测试按真实typed CallResult严格检查is_error再解析text，不改MCP/业务协议。旧失败日志保留，不能把旧结果改称本34后继通过，也不能称新4个原链case已经完整跑通。


测试夹具现在按真实版本显式分界：默认34只验完整public格式，不再重复手装；真正旧输入由历史33原链初始化并走正式保护升级。旧33里只删除表/marker却遗留完成cache、run起点凭证或Secretary案型也会在私有预检拒绝，不能靠available分支跳过格式或借同号补装。冻结03的33+显式模块库只保留原隔离证据身份，不作为本次生产输入。
