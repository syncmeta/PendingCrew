# M4 Native15 完整生产入口、历史与恢复：架构裁决方案

状态：方案及现有证据。没有新增 SDK 探测、Mod stage、模型调用或 schema15 激活；Native unavailable。此文不替代整体 M4 验收。Core14 冻结修复已由独立质量接受，原生分支已安全吸收 `1eda24dc`；其新原生增量继续隔离。

## 结论与建议裁决

现有 Core 可以可靠地保证“由 Core 发起的自动 producer 调用先有耐久授权”，也可以保证未知结果不重投、原进程 Stop、手工 custody 和原账恢复。**目前无法证明“任意原生入口最终正文进入执行前都必须通过 Core，且 skip/超时/卸载不能绕过”**。外层 ACP/真实 PTY 只能管住它实际拥有的输入及进程生命周期，不能把插件内部 API、原生 continuation 或引擎自动跳过钩子的行为改造成原子阻断。

建议保持完整目标与 Native unavailable，推进两个互不替代的证据单元：先完成已提出的 SDK06 原生历史/权限格式取证；另准备真正 producer/admission/correlation 的隔离故障矩阵，按具体新动作审授权。两者成功也仍须做整个生产 factory/ACP/restart/load/混合路由，不直接开 capability。若最终证据证实缺少不可绕过的入口，提交人类选择支持该保证的原生接口/版本或另一明确执行约束；不能把声明不支持、raw-only、hook best-effort、事后 Unknown 当作完整交付。

## 固定依据及证据层

实际声明均来自原隔离 scope：
`/private/tmp/pcw-M4-metadata-human-scope-5xexpvh8/owned-metadata-mod/.claude-plugin/types/claude-code/index.d.ts`，621847B，SHA256 `ea634205532548aa90020ff3a941e570bff5bec8ae242fb96d09dbcc8fc1bfad`。
完整原 ABI 记录是 [native-sdk-2.1.296-abi-contract.md](2026-10-14-native-sdk-2.1.296-abi-contract.md)。下面是对现有材料的裁决归纳，不是新版本调查。

| 声明位置 | 实际约束 | 不能推导的保证 |
|---|---|---|
| 9021–9101：PromptSubmitArgs/Input | 插件提交由引擎赋 origin；idle 的插件输入不带原生 turnId | Core UUID 不等于原生 prompt/turn ID |
| 9105–9148：PromptSubmitResult | 返回 entered/queued text/context/origin 或 drop；返回时可能已经进入会话/排队 | submit 返回不等于完成，也不是进入前可撤销的最终正文 ACK；结果没有 promptId |
| 4098–4110：prompt.submit hook | 失败插件可能被跳过，catch 是保护路径 | 有 hook 或 `.catch` 不能无条件证明必须阻断 |
| 5098–5133：HookBudget | own budget10s、catch1s、signal 后 linger5s；超时后 absent/next 可继续 | Core kill/watch 在观察到失败后不能撤销已发生的原生效果 |
| 832–861：BaseHookInput | native session_id、transcript_path、可选 prompt_id；agent_id 区分子代理 | 生命周期事件可能没有 prompt_id；路径字符串不是来源权威 |
| 13379–13402：TurnStartInput/Result | 最终 text 在 submit 与设置钩子后；引擎铸造 turnId，返回值不能改 ID | 不能用输出文本/本地序号伪造 native ID；此事件也不是已证不可绕过的 gate |
| 13280–13375：turn.complete | 原 turnId、reason、agentId、真实 usage（可缺） | 子代理/旧 generation/无匹配开始的完成不能结算主工作 |
| 2927–2938：turn.abort | 要求真实活动 turnId | Esc 或 Stop 进程不等于原生 turn cancel ACK |
| 10541–10712：session.append；11142–11304：messages | kept row pinned UUID；API latest≤4096，无完整 cursor | 观察序列连续或读到少于4096行不证明完整历史 |
| 7505–7524：classic.PermissionRequest | 原生回调带 tool/input 与 base context，缺 tool-use ID | 相同 tool/input 的 hash 不能当唯一 continuation ID |

现有实际证据继续保原范围：

- metadata05 receipt：`/private/tmp/pcw-M4-metadata-human-scope-5xexpvh8/attempts/05-interface-reviewed-rpxtodcl/receipt.json`，实际 native 身份/version、Unix双ACK、0stdin、mock请求空、own group reaped。没有 producer/history/approval 执行证明，不重跑。
- Native15 fake01/02：`/private/tmp/pcw-M15-owning-core-full-chunk-fake-{01,02}/result.json`。真实 kernel-enrolled proxy/Core/FIFO 和原 PTY/process custody；合法1MiB分帧完整后才 ContentAccepted；半正文 cancel/gap/foreign 不 ACK、不推进、禁止 remainder、断连 Unknown、registry清理。**proxy回显的 synthetic structured event 不是实际 SDK 原生 consumer 或 producer。**
- Store projection7：fake01/store.log，包括 owning Store/savepoint、>4096 synthetic cut、缺口/未知块拒、恢复历史化、runtime删除级联。不是 native 文件 decoder/census/祖先证明。
- SDK06 proposal：manifest `8c84dedf43f3343274a1f42dbe59ac1674b24f8cc102cd3917724ef3b3a278be`，supervisor `6f674b13554d3417c55814a071286e7f9467be21d8c40495234a428b296182a8`。目前只经 source/control 审查，**人类未授权实际执行**。该范围5001 append/latest4096/故意漏hook/一次 denied工具/source FD cut，不含 prompt.submit，不能独解 producer。

## 完整模块必须保持的不变量

1. **耐久 admission 先于自动 API 调用。** WorkBook/current actor/crew/session、真实 generation/native birth、权限、stop floor 和预算均重核；原 request/body/hash/输入 revision 写现有唯一 receipt；只有该 Tx 的 ACK 才能把一个 intent 从 Core 交给 producer。无 ACK/未知/历史 request 不可调用 native；排队不是送达。
2. **有效正文身份不可降格。** 输入重写、context、最终 native text 的含义必须明确。当前原有 exact-content 契约不能悄悄降为“只保证调用参数”。最终正文有合法来源、真实 prompt_id/native turnId 和 request 归属后才能认 ContentAccepted/Running；猜文本、单独 bridge seq、TUI 或 source行号都不充分。
3. **一轮只有一个可归属的自动输入。** Core 选定的未结算 intent 不盲重发；相同正文的相邻输入仍需不同真实原生关联。user raw 先持久 manual floor 再物理写；人类键入不继续算自动 request。native continuation 必须归属原授权运行，不能借空 text/旧 promptId 新发工作。
4. **完成来自匹配的实际原生生命周期。** genuine start 与 complete 绑定 main loop/native session/generation/birth/turn；未知、subagent、late start/cancel 后完成不得复活已撤销自动 authority。TUI/screen/watch 不结算。
5. **history 是单独来源投影。** schema15 严格14→15；12/13/14 DDL保原。原生 source SDK/session/runtime、完整 census、稳定 cut、父/分叉/压缩 ancestry 成为 private VerifiedSourceCut 的构造条件。hook缺失可发现，未知媒体/块无损保存但拒 incomplete load；不可静默截断。ACP load 必须先完整 ordered updates 再 response。
6. **审批 continuation 必须确有其物。** 每次 genuine pending callback 由 Core生成 callback identity，绑定事件序列/birth/generation/native context。receipt按当前 actor/scope/request 再核；耐久答复后仅一次调用对应 continuation。缺 native tool-use ID 时，不许靠 tool文字/hash 猜。超时/断连/重复/撤权保持未知或 deny，不增加 rules/session-wide/bypass。
7. **取消、停止、恢复分别记账。** cancel 使用真实 active turnId；物理 Stop核原birth消失；新 generation 不承接旧 unknown 输入。restart/restore/import 清当前 source/信任与执行 admission，历史 receipt/cut只作归因，不重投。完整历史重验后才能 load。不得把重启和同名session当新授权。
8. **公开 profile 来自实际 factory。** executor.list 的 required crew scope/现有 grant 和 Human admission 全核；id/能力/reason稳定不泄auth/env/path。沿 acp:<id>。Native profile 只有 producer、审批、历史、cancel/Stop/恢复全门禁齐才available，单个 fake 或 metadata不启用。

## 可证明路径分析：ACP 与真实 PTY 保持什么、缺什么

### A. Core 唯一 producer：可继续研究，不依赖 observer 作为首授权

Core 可以先耐久 admission，再由 kernel-bound owned Mod 的有界 pull/API 调用执行。只允许一个 Core-owned pending intent；真实 PTY继续由Rust拥有，公开 raw/resize沿14手工receipt/floor。对于**这个受控调用**，Core自身能实现 ACK前零API派发、未知零重投、精确参数、不跨scope。此路径保留真实PTY，不用Codex内部通信，也不靠hook计数授权。

但要达到完整保证，还必须证明 managed Native profile 的**所有自动入口**都归入这条路径；不能仅靠自述“只有我的插件”：原生恢复/continuation、scheduled/peer receive、插件卸载/重入、用户设置重写、queue、工具API、启动首轮都需要实际入口覆盖。必须获得不可绕过的 final-content admission 点，或证明整个具体 profile 中 final text/context不可被改变且所有效果确实在 Core的耐久意图之下。现有声明尚不能提供这个封闭性证明。不得凭 in-flight 单例加 body equality 猜原生 prompt 关联。

### B. 外层 ACP/PTY fence + watchdog：必要，但不足以替代 hard admission

PTY控制可以保证自己不写未经授权 bytes；nonce/kernel child/custody保证是自己的 native，Stop保证该原birth已消失，manual floor保证自己不继续自动投递。它看不到/不能原子阻断 native进程内部API以及native自动 next。SIGSTOP/TERM、有限watch、callback timeout只能保后续边界，不能撤回已经越过gate的native效果，也不能靠检测后标Unknown证明“先许可后effects”。所以B只作A的执行/恢复外壳，不作为绕过当前限制的替代。

### C. 真正不可绕过的原生入口或全 effects broker：需要明确新能力，不擅自实现

若SDK提供正式 hard entry/consumer handshake，必须验证该point在每个入口都早于effects，missing/timeout/unload会阻断。若采用独立 model/tool effects broker，必须真正覆盖网络与本机工具等所有效果、最终typedbody/source与credentials权限，不能仅代理HTTP模型请求或靠observer kill。它可能影响原生provider、签名/sandbox、权限和产品配置，是新的明确架构/授权单元，不是当前私有Unix回显组件自动带来的能力。现阶段没有足够支持依据，不建立此broker、不改real账号、不推断CLI flag可做到。

## 源码可解、取证需求与人类待决的分界

| 分类 | 完整交付单元/下一步 | 冻结判据 |
|---|---|---|
| 现有源码可解 | owning Core/FIFO admission、分帧、unknown/noReplay、custody、history投影、ACL/profile closed DTO、ordered ACP replay/restart限制、genuine callback once状态机 | 成套source/fake；必须继续保Native unavailable，无实际格式就不能构造完整cut |
| SDK06具体授权待答 | native raw rows/UUID/source cut/current suffix、故意hook缺口及genuine permission denial | exact已审原scope实际receipt/returned outcome/源hash/kernel cleanup；只解决这批证据 |
| 需要另列具体无模型取证，不默认执行 | producer submit/queue/rewrite/drop；故障beforeNext/afterNext、catch timeout/unload/re-entry；真实prompt_id/turnId/main-loop/continuation；重复相同body；missing ACK/cancel/Stop/restart。使用假HOME/凭据、localhost zero-forward、固定text、定额预算及verified-owned cleanup | 每个实际调用API和expected observer/source变化先冻结scope；typed结果与mock请求/原生事件交叉，执行unknown保原不重试；无付费model或真实profile |
| 如实测不存在 hard-entry 能力才需人类决定 | 支持该保证的Native接口/版本、或有明确全部effects约束的配置/平台方案；也可明确暂不交付Native但不能算M4完成 | 不减少已定mandatory/exact-content/完整history目标；不同产品保证必须人类明确改定，不能作者默认为manual/best-effort |

**现在可裁决的选择**：同意沿A作为研究候选，保B为已证必要外壳；SDK06与producer故障/correlation是分别审批的实际单元；C保必要能力调研/产品待决，未经具体决定不实现。若实际producer门槛无法证明，及时提交真实阻断，不用更多 synthetic greens 或 repeated metadata 掩盖。
