# 后端与客户端组合验收（2026-10-13）

工程候选 codex/work-identity-20261013，冻结门禁代码 03108f9c；Rust 1.94.0，锁定依赖、离线、all features。格式/lint/全量测试通过：2436 passed、0 failed、8 ignored；79个测试结果块只是验证统计，不是工作量。日志 /private/tmp/pcw-program-integration-gate-20261013-02.log。首次组合门禁因新 work.patrol 未进入 CLI every-method 实际调用覆盖而失败，原 -01.log 保留；b11b7abc 补实际双工作项分页、事实与详情读回，未降低覆盖清单。

本轮用户场景：人显式读取有限巡检页，获取稳定全局 W、无人负责、真实恢复失败/既有失联规则、当前升级与等待事实，并读回详情。首实现2ed9f9c1的输出有界但计算重复全账本扫描，独审捕获后1382e258使用共享纯规则按页内项/关联依赖/问题投影。权威账本恢复/校验固有成本保留；未扫描范围和未评估类别明确返回。数据库无副作用、socket/CLI、核心规则等价均已验。不调 Jev、不启动行政、不自动通知/重派/关闭；这不是行政处完整交付。

客户端草稿、握手/重试/最终响应排空修复同步进入此树，新增故障夹具已通过六场景。首版独立交付树另有当前包 fe9ef80e 和相应组合验收，完整M4b不作为其前置。共享 main、用户 README 和既有ACP/design未跟踪资料未被此轮整合修改。当前记录不证明安装、迁移、真实模型、跨厂商账户或设备验收。

下一纵向单元依据原C4：provider原始 usage 快照耐久留存与按需读回。定向方案见2026-10-13-usage-evidence-plan.md；当前仅压缩策略读数，不是成本账。新增实现应保留缺失/零和未知字段，不累计叠加thread total快照、不发明费用换算；不进入Jev/记忆选型。


## 2026-10-14 完整本机源码进入共享 main

已验收的M1工作/恢复和M2完整工作台源码，连同配套构建/迁移/验收工具，于本次以fast-forward从cd14d07cc71629a8c3976971d5438d421654f44d整合到6743003576cee5a3d92cf42b963289d239975039。普通git push origin main成功，随后实际ls-remote确认同SHA。未强推、重置、安装应用、切换日常数据或发布。

整合输入包括M2独立验收、正式schema9→10迁移与正常后台typed客户端贯通、a9c3014d完整候选来源验收，以及原主工程本地Mac/iOS Simulator构建验收。main上的GUI/Rust/构建输入相对a9c3014d无变化，不为文档提交重复构建候选。Mac xcresult是3202 total/3187 passed/15 skipped/0 failed，既有HelperBuildPerMember另外明确排除；本地Xcode27结果不能替代远端26.5。

人类README/design/历史ACP路径在整个合入diff中无变化，git dirty状态合入前后完全相同。187个合入前文件hash中186个逐字节相同；唯一design/prototypes/.DS_Store在并行时间窗变化，未推断原因、未恢复或覆盖。原始记录保留/private/tmp/pcw-M3-main-human-preservation-before.json与pcw-M3-main-integration-local-01.json。

本SHA真实远端CI已启动：CI run38457727754（工程同步已通过、Xcode26.5 Mac/iOS任务运行中），crewd run38457727840（Linux/macOS任务运行中）。这些运行中的job不能记为通过；最终结果继续由根负责。原先main旧SHA的失败记录仍保留，不因本地新验收覆盖历史。

完整窗口操作验收、当前版本真实账号使用、安装/日常迁移仍是独立关口；源码合main不代表M3全完成。后端已在独立工作树推进M4，客户端接M6既定远程/移动工作流准备，质量线继续当前窗口验收。


### 远端Linux实际失败，交付关口保持未通过

main67430035的crewd Linux job115504649566已于2026-10-13T19:54:19Z终态failure。实际nextest为2533 run/2529 passed（含1 leaky）/4 failed/8 skipped；完整原始日志保留/private/tmp/pcw-M3-linux-ci-67430035-failure.log。失败为orchestra_restart休眠回执/重启，以及driver output_drain投递最后实际字节、排队各阶段新dialog/输入权限、真实WouldBlock后新dialog撤销三个流程。个别重试成功不代表整体门禁通过。根已将完整跨平台交付缺口交生产作者，质量线接独立复验；不直接skip，不将M4未验源码混入修复。

两个gh watch观察进程曾因api unexpected EOF退出；重新读取原run事实后确认CI Mac/iOS job115504648970与crewd macOS job115504649726仍in_progress。观察进程退出不是工程任务终态，也未重新启动远端任务。旧main cd14的Swift失败日志另保存pcw-M3-prior-main-ci-failure-cd14d07c.log，仅供环境/历史比对，不覆盖本次结果。


### 同一main版本远端完整门禁终态

crewd Mac job115504649726 success：2533 run/2533 passed/8 skipped，随后doctest通过，原始日志/private/tmp/pcw-M3-macos-rust-ci-67430035.log。Linux四项失败仍未关闭，不以Mac通过外推Linux。整个crewd run38457727840结论failure。

原工程CI run38457727754终态failure：工程同步job通过；Xcode26.5 job115504648970在Mac测试阶段失败，iOS步骤未执行。当前原始日志/private/tmp/pcw-M3-main-ci-67430035-failure.log。唯一failed case为CrewChatBottomReboundProbeTests.testPagedLazyHistoryLongMessageMovesOnlyByWheelDistance，三个断言记录行坐标NaN。作者实读offset连续有限且每步-40；失效量是observedRowY，行顶端越过500pt viewport后测量消失，是否仅夹具前提或产品问题尚需实际因果验证，不能直接归因offset损坏或跳过用例。

根已把两块交付缺口分别交后台和客户端作者，在精确main674独立修复树推进，质量线独立复验。后台固定parsed请求与deadline的registry重核验，PTY测试采用真实读取握手；客户端验证实际AppKit滚动坐标与lazy行生命周期。M4和M6在途改动保留各自树，不混入M3修复。冻结修复后先推独立分支跑真实双平台/Xcode26.5门禁，通过并独验后才再次整合main及构建对应候选。当前M3保持未完成。


### 隔离修复复验与当前整合条件

| 精确源码 | 实际远端验收 | 结论与下一动作 |
| --- | --- | --- |
| 37d199f4054faa5fae7271987473f0597aeb5f1a | 原工程CI38461773955，Xcode26.5同步/Mac/iOS均success；日志pcw-M3-main-ci-37d199f4-green.log | Mac3202总计、15skip、0failure，iOS build成功；质量线已独审保持原起点与跳动反例。允许进入组合源码整合，不代表窗口点击或安装。 |
| f80e6e52c82b32537d9aed89f31b652f474444ec | CI38460564434 Linux failure，日志pcw-M3-linux-ci-f80e6e52-failure.log | 新真实失败驱动1b窄修；不盲重试相同源。 |
| 1b0c85fe1bdb2e524161a1669ce4e86163ad250b | CI38462381458 Linux success2535/2535pass、1leaky、8skip；Mac failure2534pass/1fail/8skip | 原日志pcw-M3-linux-ci-1b0c85fe-green.log与pcw-M3-macos-ci-1b0c85fe-failure.log；Mac巨大brief延迟回显用例交后台继续修，双平台门禁未完成。 |

质量线已独审1b权限固定截止/真实链/零业务副作用与25guard真实字节反例，以及37d滚动实际clip/非空中间帧/原getter突变敏感性，无额外无关全套重跑。完整门禁仍按实际远端结果，不以本地或独审覆盖失败。

窗口当前事实：实际点击0项。精确候选a9启动PID1352曾经由质量线只读ps核验；原生getApp调用2948.1秒后返回aborted by user，未返回App状态或获准/拒绝的许可结果，不能推断有人已批准或拒绝。根已暂停新窗口动作、保留独立fake应用/data/evidence，并将质量线转独立源码/运行证据验收。a9/b064包不重标新peer版本，正式修复候选须重新核验来源。没有安装、日常数据切换或新真实模型运行。

后续大块继续：后端M4真实PTY/ACP全生命周期在独立树；客户端M6共享NDJSON及权威隔离、移动原四视图操作在独立树，质量线接共享wire/取消/authority seam。移动类型检查不代表网络配对/真实手机交付，Q11 TLS1.3双向SPKI/撤销/remote actor后台依赖仍明确。M5跟进调度保留后续责任，Jev和记忆仍仅方案。


## 2026-10-14：整块后台修复冻结与新双平台 CI

- 作者冻结 `2eeec9842803bbd96e07291d8d6b62040da8e3e1`，Rust 产品输入固定于 `278ec74b`；根核对 clean HEAD 与日志。
- 本机最终 workspace05：78 suites、2547 PASS、0 fail、8 ignored；原失败日志保留。strict clippy 与 fmt 由作者实际执行 exit0，根核对日志。独立 QA 已接受此冻结范围，终端可见内容不能冒称原始字节证明；真实厂商内容 witness 仍归 M4。
- 根普通快进推送既有修复分支并实际 dispatch `crewd.yml`。运行 `38478555783` 的 API headSha 与冻结一致，Linux job `115575221003`、macOS job `115575220779`；首次核对 Linux in_progress / Mac queued。尚未通过，不以本机结果替代远端。
- 全工程开发分支已普通快进保存远端 `c1a913fc`；main 仍为 `67430035`，没有提前整合本次修复或安装新产物。
- 原始证据：`/private/tmp/pcw-ci-native-submit-workspace-05.log`、`/private/tmp/pcw-ci-native-submit-clippy-02.log`、`/private/tmp/pcw-ci-native-submit-fmt-02.log`。
- 后台线接续完整 M4；客户端/远程线继续 M6；质量线已实际重新启动接 M6 恢复信任、TLS/配对/在线撤销及共享操作独验。根接这次准确 CI 和组合交付。


## 2026-10-14：双平台修复通过并整块整合 main

- 真实运行 `38478555783` terminal success，headSha 为 `2eeec9842803bbd96e07291d8d6b62040da8e3e1`。Linux 与 macOS 均 2546 tests passed、8 skipped；Linux 单列 1 leaky，macOS 无该分类。实际 strict checks/doctests 同 run 通过，日志 `pcw-M3-linux-ci-2eeec984.log`、`pcw-M3-macos-ci-2eeec984.log` 保存在 `/private/tmp`。
- 根无冲突合入 global 为 `fe68022f3cfc9af368282d90a44e69f1d8ce8f0a`：完整 Rust 输入与通过的 2eeec984 完全相同；Sources/Tests/project.yml/Package.swift/CI workflow 与已通过实际 Mac/iOS CI 的 37d199f4 完全相同。
- main 实际远端核对原为 67430035；protected README/design/旧ACP资料在整合 diff 中没有变化。实际 inventory166 个 modified/untracked、非 ignored 文件整合前后 SHA/link 均相同。该数字不冒称 ignored 文件 inventory。两份核对记录位于 `/private/tmp/pcw-M3-main-human-before-fe68022f.json` 和 `pcw-M3-main-human-after-fe68022f.json`。
- 根快进本地 main 并普通推送远端 fe68022f，自动组合 CI `crewd 38479786302` 与 `CI 38479786399` 均绑定此精确 SHA，仍待完成，不手动重复 dispatch。main 合入不代表安装、窗口、真实 provider 或整项 M3 完成。
- 新 normal helper 输入已 prepare（尚未 build）：`/private/tmp/pcw-M3-normal-backend-20261014-2eeec984`，RustInputDigest `c102588bdde3cae28abc5b8a8849c8fe8fb23a4a39fec8dbc3aaff4f408c016b`。旧 b064 helper/a9候选不冒充新源码；等待唯一 target 短窗口，再实际正常构建及新 GUI/helper 配对。


## 2026-10-14：新 main 完整正常后台/GUI候选实际构建

- 后台作者明确交付唯一 target 短窗口后，根实际执行 normal `cargo build --offline --locked -p crewd --message-format=json`，exit0；Rust1.94、default features、debug、非 all-features/非 test artifact。完成 witness `/private/tmp/pcw-M3-normal-backend-20261014-2eeec984-witness.json`。窗口已明确归还作者与 M6/QA 协调。
- GUI 固定 main `fe68022f`，复用上述完整输入相同的实际 helper，经 immutable snapshot、Swift -O 实编译、打包和 adhoc 签名生成新候选：`/private/tmp/pcw-M3-immutable-candidate-20261014-fe68022f-02/PendingCrew Codex First.app`。
- 首次构建调用遗漏 explicit debug/toolchain 意图，门禁正确以配置不匹配拒绝，原路径与日志保留；补足 Rust1.94/debug 意图后新目录 build02 成功，未重复编译 Cargo。第一次 artifact 检查误传容器目录而非 .app，立即按脚本参数纠正；不是产品失败。
- 根实际 artifact checker PASS：完整 GUI/Rust 输入、pre-sign binary、实际配置/日志、打包 digest 及签名配对；日志 `/private/tmp/pcw-M3-immutable-candidate-fe68022f-build-02.log`、`pcw-M3-immutable-candidate-fe68022f-artifact.log`。已交质量线独立产物验收，尚不宣称独验已通过。
- main 自动组合 CI 仍待结果；没有启动新候选、没有新窗口点击、真实 provider、安装或日常数据切换。M3 仍未完成。


## 2026-10-14：新 main 候选独验收口

质量线实际 artifact checker exit0 且独立从 Git object 比对：369 tracked GUI/Rust 输入匹配 fe68022f，349 Rust 输入还匹配实际 helper witness 2eeec984；配置/原始日志/pre-sign/打包 hash/strict 双签均通过。生成 CandidateMain 单列。质量记录91d2b919已整合为44489e7b；纯追加文档冲突完整保留旧窗口及 CI 段和新独验段，未覆盖历史。实际证据 `/private/tmp/pcw-M3-fe680-artifact-independent-01.log` 与 `.json`。这只关闭新候选的产物独验层；main组合CI、实际窗口、真实任务、安装/迁移仍分别待验。

已绑定 main 自动真实 job：Swift/Mac/iOS 115579275488（run38479786399），工程同步115579275642已通过；后台Mac115579274922与Linux115579274975（run38479786302）。两个 run 精确 SHA 均 fe68022f，仍 live。观察器 session91790/81118 对应这两个 run，不重新 dispatch。


## 2026-10-14：main 后台双平台已通过，独立模块继续

真实 main run38479786302 已 terminal success，SHA fe68022f；Mac115579274922与Linux115579274975均2546pass/8skip，Linux另记1leaky。完整日志 `/private/tmp/pcw-M3-main-macos-rust-ci-fe68022f.log` 与 `pcw-M3-main-linux-rust-ci-fe68022f.log`。Swift run38479786399仍live，根保留原watcher91790。质量线接实际新normal packaged helper＋typed客户端受影响组合验收，不重跑稳定单元全集或启动GUI/真实模型。

M6 source-only接线首次被自动审查以持续网络暴露/具体授权不足拒绝，未执行。作者保留同动作，提交准确的隔离源码范围/default-off证据后复核通过，并未换代理或工具绕过。根实际只读核对 schema enabled=0，start_persisted 在disabled返回且先于identity/listen。证据 `/private/tmp/pcw-M6-source-only-routing-scope-proof-01.txt`。这只允许既有范围的源码实施，未启动LAN或改日常配置，模块仍须完整服务/TLS/配对/撤销验收。


## 2026-10-14：main 组合 CI 全部关闭

根实际观察 Swift watcher91790 terminal exit0，并以 API 再核对 run38479786399 terminal success、headSha fe68022f。Mac 主工程实际3202总计/15既有skip/0failure（3187通过，既有HelperBuildPerMember单列CI排除），iOS实际 BUILD SUCCEEDED；工程同步job115579275642通过。完整原始日志 `/private/tmp/pcw-M3-main-swift-ci-fe68022f.log`。此前同 main Rust run38479786302已实际Linux/Mac全绿并保存原日志。所有本次main组合运行已terminal，不再重复调度或轮询这些handles。

该结果关闭本次源整合与平台CI关口，配对正常helper/GUI产物独验也已通过；受影响normal-runtime typed组合由质量线接续。实际GUI点击、真实账号任务、安装/日常数据切换及持续自用分别仍待完成；不以本次CI把整个M3或全工程宣布完成。M4原生真实PTY/ACP与M6可信远程/移动端并行继续。


## 2026-10-14：正常配套运行验收与新窗口材料

- 质量线整块新normal packaged helper＋固定fe68022f typed客户端验收已通过，原提交ba39cee4已接为27d85118。实际群/#W/H、真实MCP身份、只读重连与旧UUID不重关联均GREEN。新增物理proxy在真实已提交todo.create成功后截住回执并shutdown连接，保留原unknown UUID/完整草稿；只读reconnect零自动写重放；显式原UUID确认只保留一份权威需求且不清新稿。根核对两个实际日志及集中记录，没有复跑稳定单元或模型。
- 实际签名helper SHA238bad7f35e707e21e4c31cebb76d0569de2405baf025f025f41b62283fa02dc；正常源码/配置来源为已验收2eee，与旧b064分开。证据 `/private/tmp/pcw-M3-fe680-channel-normal-independent-02.log`、`pcw-M3-fe680-committed-eof-independent-02.log`；完整自有DB/日志/请求留在各临时evidence目录。
- 根核对既有实际fake copy SHA0e75fcbc0ff7e9f8bdb59bc41e8b937d3631317cb6dcba23581eaf64909e3d3e，使用已接受prepare工具为新fe680完整候选准备独立窗口材料：`/private/tmp/pcw-visible-gui.cd9aj8ec/session.json` 与待启动脚本。fresh home/work/data、固定GUI/helper/fake hashes；只prepare，没有launch或App控制。旧a9暂停窗口及全部证据保留。
- 新正常运行/源整合/全平台CI/配套产物关口已接受；实际新窗口点击、真实provider任务、安装/日常迁移仍待完成，不缩减M3或全工程范围。
- M6正式服务源码检查点d17766af已默认features定向check/proto通过，并合入公共2eee；QA接实际临时loopback生产路由整链，根不提前合main。客户端继续Native配对/Keychain/TLS/Root，后台继续M4，不等待离线用户窗口权限。


## 2026-10-14：后续完整模块与非阻塞人类边界

M6生产路由检查点d177已普通快进保存远端开发分支；M4 Core/native双进程登记检查点13c2a8b7已保存独立远端开发分支。均不提前合main或宣称完整M4/M6完成。M6质量线继续真正public backup/restore旧cert与旧accept UUID墓碑、attached身份拒绝；客户端接原生可信移动链，后台接完整bridge/ownership/lifecycle。

资源回收已有两份明确清单：QA15个terminal显式Swift/Clang缓存释放0.904GiB；后台41个旧静态rlib释放1427738712bytes，保留每crate最新8且旧>1h。源/原日志/DB/图/实际二进制/witness/DD/xcresult保留；目标仍唯一。该资源用于后续原生平台门禁，不新建冷target或重跑已关闭的main门禁。

M4 metadata原外部目标疑虑经同动作准确localhost环境证据复核通过，实际probe零mock请求并已回收，随后主题/dummy初始化到workspace-trust新边界。该信任动作CreateProcess前被自动审查明确拒绝，未执行，不能换路径/工具/代理绕过。拒绝原文 `/private/tmp/pcw-M4-metadata-trust-auto-review-rejection-01.json`：信任授予read/edit/execute，超出此前不含工具/权限批准的metadata许可。

为用户可审阅的最终批准，作者只prepare固定目录 `/private/tmp/pcw-M4-metadata-human-scope-5xexpvh8`；根已读4个只观察文件、完整固定88a9 supervisor与prepared-scope.json。实际preparedOnly=true/CLI未启动/mock未绑定/awaitingHuman=true。根已提交非阻塞人类问题，只批准这次自有目录、dummy凭据、localhost mock、无模型/真实auth/日常配置的workspace-trust；尚无回复，不能按推荐选项或时间流逝当批准。真实build types仍unknown，bridge其他源码工作不停，全工程目标不暂停。

## 2026-10-14：Native Mac 原目标构建与后台整合准备

Native 客户端固定源码 `1e51045a7e5de6a9b04ec0d74ea7d87091e0c19d` 的原 Mac 目标实际构建已 terminal exit0，日志 `/private/tmp/pcw-M6-native-main-macos-build-02.log` 为 `BUILD SUCCEEDED`；结果 `/private/tmp/pcw-M6-native-main-macos-build-02-result.json` 记录 clean 输入与构建后不变。根独立逐项将输入 manifest 的356个文件 SHA 与该 commit 的 Git 对象核对，零差异，结果 `/private/tmp/pcw-M6-native-main-macos-build-02-root-input-check.json`。此证据只关闭该源码的 Mac 目标构建，不替代 iOS 构建、Native TLS 运行、设备、配对产物或安装。

远程后台 `d17766af` 相对已验 main `fe68022f` 的22文件范围已在 global 树实际 `git apply --check` 无冲突；仅预检，未应用或合 main。后续只在服务完整冻结后整合 schema11，供 M4 schema12 接续。当前后台后续修复 `2d0c445e` 保持未配对 grant 列表只读；作者继续慢 writer 撤销与有界发送，质量线继续实际反例及正式 schema10→11/旧版拒写验收。已接纳业务完成与 socket 部分帧取消不得混为一项，也不以丢失回执后耐久事实仍在的通过替代慢连接撤销证明。

## Native 实际 Xcode26.5 CI：保存失败，先同步已验基线

实际 workflow_dispatch `38488956854` 精确源码 `778e5d5c4699a023319ae808ef9d8d960aca5bd6` 已 terminal failure，唯一 observer55258 exit1，不再轮询。工程同步 job115609340590通过；build/test job115609341157完成Mac编译，但3202测试/15跳过/3断言失败，全部来自同一 `testPagedLazyHistoryLongMessageMovesOnlyByWheelDistance` 旧NaN测量；后续iOS步骤未执行。原始失败日志 `/private/tmp/pcw-M6-native-ci-778e5d5c-failed.log`，观察日志 `/private/tmp/pcw-M6-native-ci-778e5d5c-watch.log` 全部保留。

根核实 M6 没有 main 已验 `37d199f4`（实际祖先检查exit1），对应测试diff正是旧离屏lazy行测量，不能当新增Native产品缺陷或删测。根派CI前漏核该基线；作者直接同步已经通过真实CI的修正/已验main `fe68022f`，继续自身模块。新完整冻结后再派平台CI，不能盲重跑旧ref。后续组合冻结必须核对最新已验main及相关修正实际包含情况，再判断必要新门禁。
