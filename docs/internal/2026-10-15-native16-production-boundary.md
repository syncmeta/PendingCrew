# Native16：Jev 冻结后的完整生产边界

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
