# Native16：Jev 冻结后的完整生产边界

## 2026-10-19 真人产品裁决：受管任务边界

真人已选择“采用受管任务边界”。本节覆盖下文历史方案中把全部原生内部入口、最终正文和全部 effects 强制纳入 Core 原子准入，以及把自制 OS canary 资格作为受管任务必选前置的要求；其余历史记录保留作来源，不作为继续等待该 hard-entry 的理由。

Core 对自己发起的 dispatch 保证：当前 Human/Agent 与 crew ACL、当前运行资格和 generation 先核；唯一耐久 intent 先于实际派发；真实 PTY 与 kernel birth/custody 归属；只有真实供应商结果/事件才推进相应事实；断线、丢回执及重启保留 Unknown 且不自动重发；重开不自动恢复 session。取消请求、供应商取消结果与原 birth 的物理停止分别记录，不声称撤销已经发生的外部效果。

Claude 原生工具、插件及 continuation 在本次获准运行内按供应商原生权限处理。PendingCrew 不承诺对所有内部入口或最终改写文本逐项事前拦截，不将观察 hook 当不可绕过的安全边界，也不自建全 effects broker。

完整历史与手动恢复是独立能力：未经真实来源与完整覆盖验证，不宣告 full-history load，不以最新 4096 行、屏幕或连续 observer 序列冒充完整历史。新建任务与有限事实回执可以独立于该能力实现；旧 Unknown 不因新 generation 获得重投资格。

本裁决授权源码及必要既有 fake/本机组件验证，不授权新的 SDK、模型、OS canary、真实 provider 或 App 操作。源码实现与模拟验证不等于已取得真实 Claude/ACP 兼容资格；公开能力仍只按实际已验证范围注册。

## 2026-10-19 开发分支保存边界（不是 M4 完成）

当前源码加入正式 Human `executor.list {crewId}`，经现 peer/TLS grant、当前 authority 和 crew ACL，只返回闭集 profile/features/reason，绝不返回程序、HOME 或认证材料，也不初始化 provider。`--claude-managed <PROGRAM>` 是明确真实 Claude 程序配置，不是外部 ACP wrapper；仅作 FS 元数据判断。`acp:claude-managed` 目前只登记 unavailable 元数据，**没有注册可启动的 Executor**：即使文件可执行，reason 仍是 `nativeVerificationPending`，全部 features 为 false。旧 `claude` PTY 与 `codex` 实现不因此成为 Native ACP。

受管 API 资产使用同一 Core DB 的 `native_managed_operations` 作为唯一 intent/API 结果账；旧 `native_execution` 强原生 ID/one-open 语义保持，二者不双写同一 intent。准备、派发、真实 API 返回与 Unknown 分开；返回不标记原生 turn 或工作完成，重启/恢复将未决项留为 Unknown，不恢复派发资格。当前 bridge 和 bundled Mod 是**未注册资产**，尚未接成完整生产 ACP 工厂，不宣称实际审批/取消/派发兼容已验。

唯一标准 ACP 阻断：已保存 2.1.296 类型与官方 Mod API/events 没有给出一次 `prompt.submit` 调用的 submission handle/native prompt ID 到对应主 turn ID、再到完成事件的受支持关联。返回 text/context/origin/drop 只说明接收或排队；plugin origin 不含每次 operation ID；可选 classic prompt_id、append 行 UUID 与 turn ID 未建立可证明映射。不得靠同文、首 turn、单 pending 或 TUI 静默补联。私有 API 扩展不冒充标准 `session/prompt` 完成，不编造 `stopReason=end_turn`。缺完整来源的 `session/load`/history 能力独立保持不可用。

此开发分支的 Store 默认格式是 **Core16 fresh-only**：`Store::open`/`open_data_dir` 前置检查会 typed 拒绝既有15及其他不支持格式，保留原文件与 sidecars，不做15→16迁移、导入或静默换目录。新库复用现 bootstrap，最终事务只安装受管操作账；不安装 `native_history` 的历史 V16_DDL。此切换影响所有使用该新源码的普通/Native入口，并非 `--claude-managed` 的 opt-in，因此本轮只保存开发分支；**不能直接替换仍使用 Core15 的日用主干或旧产物**。新建格式安装失败只回滚该最后事务，失败的新私域保留，不能宣称整个历史 bootstrap 是一个原子迁移。

资源可移植边界：`crewd/crates/exec/src/acp/managed_plugin.js` 由同目录 `managed.rs` 的 `include_str!` 编进原 crewd helper；安装函数仅在新私有 run 目录以 create_new 生成 `managed-plugin/.claude-plugin/plugin.json`、`managed-plugin/hooks/hooks.json`、`managed-plugin/hooks/register.js`。不依赖开发树、private/tmp 固定源码路径、Node/npm 或额外 ACP wrapper；现 helper 仍是唯一打包输入。临时运行目录不是可交付资格证明，当前安装函数未从 profile 工厂调用。未来真实资格动作仍须单独具体授权，不在本轮运行 SDK/provider/model/App。

## 以下保留 2026-10-15 历史记录

以下“当前/必须”仅描述当时旧保证与资产，已被上面人类裁决和开发边界覆盖的部分不再作为本轮产品前置。

2026-10-15，Jev 独立提交630babe24f63d81dcda183f261bb824f8cb71c41已作者冻结并交整体独验。其正式格式为15；Native历史格式预留16，只有独立提交8aebebb864d7e73941d7a6f7aad86732a2194593改V16_DDL名称与注释，没有安装DDL。旧文件名中的Native15是历史方案，不能据此激活15/16或把Jev格式门当Native资格。本记录独立于Jev完整patch，不改变其源码或证据。

生产入口现状由实际源码决定：server/orchestra/exec.rs的Executors仅返回实际注册的实现；未知acp profile返回Unsupported。proto/executors.rs目前只有未注册的闭集DTO，不能声称executor.list已交付。server/native_receipts.rs的CoreNativeReceipts与OwnedHelperPeer提供真实custody绑定、FIFO耐久ACK、分帧与未知恢复机制；它们没有赋予原生producer能力。store/native_history.rs的VerifiedSourceCut没有公共/Mod构造器，完整cut仍须真实原生来源验证；不能直接以新4096窗口或连续observer序列创建。

## 必须作为同一个生产单元完成的缺口

| 缺口 | 实现前必需的真实输入 | 关闭标准 |
| --- | --- | --- |
| Core唯一自动producer与factory | 可用隔离执行边界；真实submit/queue/drop/rewrite结果与prompt/turn关联；全部自动入口的可证明约束 | 原耐久intent先于effect；最终正文/原生ID与真实generation/birth相符；missing ACK、skip、timeout、unload不能把未授权自动输入送入执行；没有证据就不注册available |
| 原生history decoder及完整cut | 固定版本真正owned transcript的原结构、稳定FD cut、UUID、fork/compaction祖先关系 | 全源census与ordered rows一致；漏hook可检测；未知媒体/块无损保存但incomplete；ACP完整load后才答response，不能截断假称完整 |
| genuine审批、cancel、Stop与恢复 | 真实callback/continuation身份、工具拒绝结果；真实active turnId与完成事件 | 审批耐久后仅调用原continuation一次；取消与物理Stop分别记账；断线/重启/恢复保持Unknown历史与零自动重投；旧generation不能完成新工作 |
| profile与混合路径 | 上述factory/producer/history完整资格及当前权限 | required crew ACL、本机/配对Human和method grant保持；不泄program/HOME/auth；旧Codex/Claude PTY不能冒Native兼容 |
| 正式16版本及交付 | 完整源码/假验证和必要真实隔离证据接受 | 独立15→16保护迁移/失败回滚/旧15拒写、source身份失效、新准确normal和必要两平台范围；当前不激活、不扩Jev产物 |

## 已有证据复用及明确阻断

Core/FIFO/真实owned proxy分帧与Store投影已在/private/tmp/pcw-M15-owning-core-full-chunk-fake-{01,02}通过；这些证明自有代理与synthetic事件，不是SDK consumer、真实producer或完整原生history。现阶段不为等待重复这些组件/M5/稳定全量。

实际SDK声明、行号与能力限制沿2026-10-15-native15-producer-architecture-decision.md：submit结果没有可直接当Core request的promptID；prompt.submit hook存在skip/timeout路径；turn.abort需要真实active turnId；messages最多4096且无完整cursor；PermissionRequest本身不足以推唯一tool-use continuation。维持已裁A（Core唯一producer研究）+B（真实PTY/custody/恢复外壳），不实施C全effects broker，不将manual/raw-only降格当完整M4。

TypeScript来源及strict静态层已独验5e1033b8ca4a15e8c3ce2ce554fae9de1e6bb4b516e35d2b27f09bbd7ae6a2ad；工具取得HOME/PATH偏差保留，不重下载/静态复跑。candidate04的rawError narrowing是runtime变化：undefined属性可被JSON省略，旧??null为显式null；function throw变全null。未来具体action必须绑定candidate04六个文件新hash和真实supervisor/结果判据，不能用旧candidate03或仅注释措辞覆盖。完整记录/private/tmp/pcw-M15-producer-admission-scope-preparation-01/typescript-independent-closure-record-02.json。

SDK06历史/权限真实动作仍未授权；metadata05不覆盖它。固定11case producer scope仍仅源码准备，未获真实执行授权；strict intent→nativeID未知、cancel默认false保持。OS隔离canary05真实SIGABRT且reaped，精确日志无qualified row，没有定位某项缺权限；独验cd9f02e8b2486bc9524a6fdc330e3429c2f62e68d76196d2fffb1d4bd81a2175。具名OS支持问题/private/tmp/pcw-M15-native-free-isolation-canary-05/os-support-needed.json仍待真实支持输入；不得猜Mach/file/net放宽或再跑canary/日志/SDK。

当前没有能在不伪造原生格式/身份、削减mandatory保证或重复组件验证的前提下独立完成的另一完整生产源码块。下一有效动作顺序：先取得具名OS支持/明确隔离取舍；另获SDK06具体授权后一次保留真实source/denial证据；据实际来源实现decoder/ancestry verifier；再封完整producer故障/ID动作审查。若实际接口不支持不可绕过的入口，将具体证据交根作接口/版本或产品决定，不能作者自降保证。以上不阻Jev质量修复、客户端或现有Core交付缺陷处理。唯一Rust/schema作者仍backend，当前无新增编译、SDK、模型、Keychain或App操作。
