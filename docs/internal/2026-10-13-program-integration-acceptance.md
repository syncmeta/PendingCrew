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
