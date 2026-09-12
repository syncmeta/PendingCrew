# 技术债 / 项目根基风险登记册

这个文件是 PendingCrew 的**结构性问题登记册**，由各个 Claude Code / Codex session 在日常开发中**顺手记录**，不是一次性审计的产物。

记什么（按严重度从高到低）：

- 🔴 **根基级** — 影响整个项目、让根基不稳的问题。错误的数据模型、会随规模放大的设计错误、跨模块的隐性耦合、安全/数据一致性隐患之类。
- 🟡 **拆东墙补西墙** — 为了让 A 跑通而硬塞进去、把代价转嫁到 B 的修复；绕过类型/校验/约束的 workaround；复制粘贴而非抽象；"先这样以后再说"的临时实现。
- 🟢 **不规范** — 偏离本仓库既定约定的写法、命名、结构；不致命但会慢慢腐蚀一致性。

**不记**：linter/typecheck 已经能抓的纯风格问题、个人口味、与项目健康无关的琐碎 TODO。

---

### 🟡 `recordSessionMember` 把「这条 crew 不存在」压成一个没人看的 `false`（**这一单故意不修**）

- **发现**: 2026-09-09 · 47-1 在为总机组那一层选存储方案时，验「保留 id 能不能直接用」撞到的。
- **位置**: `Sources/Stores/LocalCrewStore.swift` · `recordSessionMember` 第一行
  ```swift
  guard var crew = crews[crewId] else { return false }
  ```
- **形状**：名册里没有这条 crew → **不抛、不告警、返回 `false`**，而这个返回值调用方
  基本不看。后果是一个 session **登记不进成员表、拿不到分机号、通讯录里看不见**，
  而调用处看起来一切正常。
- **同一个文件里另一种处理**：`attachParent` 遇到同样的情况是**抛** `crewNotFound`。
  **两种处理方式并存，而危险的是静默的那一种。**
- **它属于哪一族**：把「出事了」压进一个正常返回值。这个仓库同期撞到的同族还有
  「读失败压成空数组」「没验凭据压成通过」。
- **为什么这一单不修**（这条比 bug 本身更重要，半年后回来看的人需要它）：
  总机组最终选了「名册里给它一条 `isBuiltin`」的方案，**于是这条静默路径根本不会被
  这一单触发**（名册里有它）。修它要动一个被多处调用的写入口的签名或错误传播，
  属于另一件事；扩进来会让这一单从「加一层」变成「顺手改数据层」，而这一单的
  验收面本来就已经很宽（四处组织操作的守卫 + 一层新界面）。
- **修的时候要注意**：调用方是 `CrewSessionRunner`，它在 session 生命周期里调；
  把返回值改成会抛或三态之前，先确认那条路径上「crew 恰好刚被删」是不是一个
  合法状态 —— 如果是，就不该抛，而该有一个明确的「这条已经没了」分支。

### 🟡 「测试文件不许 `@testable import PendingCrew`」这条规矩**没有任何东西在执行**，今天复发

- **发现**: 2026-09-08 · 修 `withdraw_human_todo` 那一单顺手撞上的。
- **形状**：`PendingCrewTests` 是 standalone bundle（`TEST_HOST=""`，源码直接编进
  bundle，`project.yml` 里那个 target **没有 `dependencies:`**）。所以测试文件
  **不该** `@testable import PendingCrew` —— 这句话白纸黑字写在
  `Tests/PendingCrewTests/DirectoryWatchCoalescingTests.swift` 第 2 行的注释里。
  写了会怎样：**只在 DerivedData 里恰好躺着上一次构建留下的 `PendingCrew.swiftmodule`
  时才编得过**。全新 `-derivedDataPath` 一律
  `error: unable to resolve module dependency: 'PendingCrew'`，整个 test target 编不过。
- **债在哪**：**这不是第一次。** 2026-09-05 已经删过一批（`347aac5`），
  今天在 `AgentCLIMaintenanceTests.swift` 上原样复发，又删了一次（`b73ac38`）。
  两次之间**没有任何检查会发声** —— 靠的是「下一个人记得那条注释」，
  而那条注释在另一个文件里。
- **为什么它特别难看见**：写的人那台机器上是**绿的**，而且是真绿。
  假绿的来源不是测试写错，是**构建产物的残留**。红只出现在
  「换 worktree / 换机器 / CI 第一次跑」，也就是**别人**那里。
  本机挂着几十个 worktree，暖 DerivedData 遍地，所以正常开发几乎撞不到。
- **该怎么治**（下一个碰它的人）：加一道能红的尺子，不要再写一条注释。
  最便宜的形状是在既有的自查脚本里加一条：
  `git ls-files 'Tests/**/*.swift' | xargs grep -n 'import PendingCrew'`
  命中即退 1。**加之前先植入一个样本证明它真会红**（本仓库有过「尺子扫的是
  `git ls-files`、而新文件还没提交所以扫不到自己」的先例）。
  更硬的一道是让 CI / release-gate 用**全新 `-derivedDataPath`** 跑一趟 ——
  那道尺子连没想到的同类问题一起罩住，代价是一次冷编译。
- **旁证**：同一个坑在共享记忆里记作 ①e。规矩存在、被写下来过、还是复发了 ——
  **规矩失效要靠加触发点，不是靠重写内容。**

### 🔴 账本 `open()` 撞 `EPERM` 的**发出者仍未知** —— 这次只补了诊断，没治病

- **发现**: 2026-09-08 · 计划 #48。根因分析在
  `docs/internal/2026-09-08-ledger-read-failure-diagnosis.md`，
  前置现场在 `docs/internal/2026-09-05-whiteboard-oneway-outage-capture.md`。
- **形状**：`~/Library/Application Support/PendingCrew/whiteboards/` 下的账本文件
  会**间歇性读不出来**（9 天里落在白板上 10 条系统警示、跨 7 个 crew，1~3 次/天）。
  9-05 那次抓到现场，测到的是内核层 `EPERM(1)`：**元数据能读、能写、就是读不了内容**，
  一整棵 `Application Support` 子树里 23/128 个目录同时被拒，**只打 claude 谱系进程、
  app 自己同时读写正常**，持续约 8 分钟后自愈，内核一条日志都没有。
  已经被数据打掉的候选：ACL / flags / provenance / 目录名 / 同 bundle id 两份签名 /
  agent 权限层 / 按文件缓存 / 按 crew 分隔 / tccd `Failed to match existing code
  requirement`（换尺子重算过两次，都对不上）。
- **债在哪**：**2026-09-08 落的那一版只是让它下次发作时可诊断**（errno + 绝对路径 +
  pid + argv 进消息、并落一份只写不读的 `whiteboards/diagnostics/read-failures.log`），
  **病本身一个字没治**。数据是安全的（8-12 那条「读失败永不销毁原件」的不变式挡着），
  但每次发作都会：① 往群里丢一条吓人的系统警示；② 在途的 `post_to_crew` 真的发不出去；
  ③ 让一个 agent 的那一拍白干。
- **下一步的入口**（写在这里免得下一个人从零开始）：
  1. 发作时先看 `diagnostics/read-failures.log` 的 `argv=` —— 它一句话回答
     「是 claude 谱系被拒（同 9-05）还是 app 自己也读不动（另一件事）」。
     **这个判据以前不存在，是这次补上的。**
  2. 9-05 那份文档 §9.2 留着一个还没有模型罩住的矛盾：三条谱系**起点散 36 秒、
     终点聚 31 秒**。「每进程独立重评估」解释得了发散、解释不了收敛；
     「一个全局开关」反过来。**别挑边，那个矛盾本身就是线索。**
  3. 有一条弱线索没做实：11 次可对齐的发作里 **2 次落在唤醒的那一秒**（偶然概率 ~2%）。
     要做实只需要一次「合盖 → 唤醒 → 立刻看日志」，但那要动人的机器，得先问人。

---

### ✅ 产品不再写任何目录信任位（**2026-09-08 人类裁定，已拆**）

- **裁定**: 产品**一个信任键都不写** —— `~/.claude.json` 的 `hasTrustDialogAccepted`、
  `~/.codex/config.toml` 的 `trust_level`，两个都不写，**迁移那条路也不写**。
  检测允许（只读），写不允许。未信任时**弹一个提示**，把要跑的命令原样给人。
- **理由（钉在这儿，免得以后有人翻回来）**: 信任的单位是路径，那是 claude / codex
  两家定的规矩，不是我们定的。人对 `/a` 点的那一下头，我们复制到 `/b`，`/b` 那份授权
  就是**我们**签的，不是他签的。「搬」这个动词听起来像守恒，其实凭空多了一份。
- **拆了什么**: 整套补种器（`ClaudeTrustSeedPlan` / `ClaudeTrustSeeder` 及其两个测试
  文件、`CreateCrewSheet` 的调用点与回执）；迁移那半的 `Action.copyCodexTrust` /
  `addCodexTrust` / `Skip.codexTrustSourceMissing` / `codexTrustTargetExists` /
  `Probe.codexTrustLevel` / `Receipt.codexTrustCopied` / `loadCodexTrustLevels` /
  `codexConfigURL`（连带备份名单里的 `config.toml` 与 `import TOMLKit`）；
  `claudeSettingsKeys` 里的 `hasTrustDialogAccepted`。
- **加了什么**: `WorkdirTrustPrompt`（只读检测 + **唯一一份**提示文案）与
  `WorkdirTrustPromptView`。建 crew、界面迁移、机长 `change_workdir` 三条路
  **共用同一份文案** —— 上一轮刚栽过「同一句话散在三处、改了两处漏一处」。
- **为什么留着断言而不是只删代码**: 「显式选择不做」和「不小心漏了」在代码上长得
  一模一样，在半年后长得完全不一样。所以 `neverWrittenClaudeKeys` 显式列着它、
  执行层即使被点名要也挡掉，`TrustNeverWrittenTests` 五条钉住这件事。
- **文案里不许写死那个信任框长什么样**：编号、选项顺序、默认高亮由上游说了算，
  2026 年 8 月底到 9 月初的 11 天里整个换过一版。有测试钉着
  （`test_文案不描述那个信任框的形状`）。
- **诚实的那一栏**: 迁到新目录后，新目录**确实**没被信任 —— 那里的第一个 session 会
  停在自己的信任确认上等人。这不是回归，是我们不再替人签字的**已知代价**，
  提示里把命令给了人。

---

### 🟡 iOS 的 LaunchAgent 拷贝没有尺子挡着，而且它躲在编译错误后面

- **发现**: 2026-09-07 · 修 #110 顺手修 main 上的 iOS 红时撞出来的。
- **形状**：`project.yml` 把开机自启那份 plist 用 `copyFiles` 拷到
  `Contents/Library/LaunchAgents`（`SMAppService` 只认这个位置）。
  **iOS 的 app 包是平的**，于是包根下多出一个 `Contents/` 目录，
  签名阶段报 `unsealed contents present in the bundle root`，整个 iOS 构建挂在 CodeSign 上 ——
  **报的是签名错，字面上跟 LaunchAgent 一点关系都看不出来**。
  已加 `destinationFilters: [macOS]` 修掉。
  **2026-09-07 晚更新：这一处的实例没有了** —— 人类否掉常驻方向，开机自启整个删掉，
  那条 `copyFiles` 相位连同 plist 一起从 `project.yml` 移除。**但这条债不销**：
  债从来不是那份 plist，是**「往 app 包里拷文件」这类改动没有任何尺子挡着，
  而它在 iOS 上表现为一个字面上毫不相干的 CodeSign 错误**。下一个加 `copyFiles`
  的人会原样再踩一次。
- **债在哪**：这一处**没有测试挡着**。同批的另一处（跨平台文件用 macOS 独占 API）
  已经被 `CrossPlatformSourceTests` 钉住了，这一处只有**真跑一趟 iOS build** 才看得见 ——
  资源/构建阶段的平台归属出错，源码扫描扫不出来。
- **比这条更该记住的一句**：**「编译错误修完」不等于「iOS 绿了」。**
  这两处是**串着**的：第一处（编译）不修好，构建根本走不到签名阶段，第二处**根本不会显形**。
  当天的判据只写了「先跑 iOS build 拿到红的原文，再修，再绿」——
  **如果修完第一处就宣布绿，会漏掉第二处，而且漏得理直气壮**（编译器不报错了）。
  所以平台修复的收工条件是**跑完整一趟看到 `** BUILD SUCCEEDED **`**，不是「那行 error 没了」。

---

### 🔴 变异自证会被**同一条判据链上更早的短路**挡住 —— 而读数是「绿的」

- **发现**: 2026-09-07 · 47-1 在给 `withdraw_human_todo` 收尾时撞出来的。
- **现场**：22 条测试全绿之后，按「先证明尺子会红」的规矩做变异自证 ——
  把被测的那一项（`LocalTodoItem.isUnanswered` 里的 `withdrawnAt == nil`）**整条删掉**重跑。
  **一条没红。**
  原因：`withdraw` 顺手落了一条写着原因的回应，`responses.isEmpty` 在那一项**之前**
  就把整个 `&&` 链的结果决定了。被删的那一项**根本没有被任何一条用例测到**。
- **为什么它比「一条从不发声的检查」更阴**：那种至少从没发过声，
  盯久了会有人问「它怎么从来不红」；**这种发过声，而且是绿的**。
  「删掉 X 之后测试不红」和「X 有覆盖、只是这次没坏」，**在读数上一模一样**。
  绿本身成了伪装。
- **判据**（做变异自证时用）：
  **确认被删掉的那一项是「唯一能决定结果的那一项」** ——
  构造用例时先让同一条链上其它项**全部不成立**，再删被测项。
  否则你测的是**链**，不是**项**。
- **本次的补法**（可照抄的形状）：加一条直接钉最裸那一面的用例 ——
  `Tests/PendingCrewTests/HumanTodoWithdrawTests.swift`
  的 `testWithdrawnMarkerAloneExtinguishesUnanswered`：
  条目**只有**撤回标记、`responses` 为空，再断言熄灭；
  并配一条反面（把标记去掉必须重新亮），否则正面那句证明不了什么。
  补完再跑同一个变异，红了。
- **未还的部分**：这只是一次人工自证。仓库里**没有任何机制**会发现
  「某条 `&&` 里的某一项没有独立用例」——
  下一次同形失效仍然只能靠有人恰好去做变异，而且恰好构造对了用例。

---

### 🔴 替卡住的成员解围：**能用的那个动作不安全，安全的那个动作没有**

- **发现**: 2026-09-07 · 44-1 想替两个停在输入行上的 worker 解围时撞出来的。
- **形状**（两半各自成立，合起来才是问题）：
  1. `nudge_session` 的语义是**发文本 + 自动补回车**，没有「只提交、不添字」的形态。
  2. 于是当对方输入框里**已经躺着一句没提交的话**时，nudge 过去的字会**接在那句后面一起提交** ——
     提交出去的是一句我没写过、也没人审过的**缝合指令**。
  3. 唯一不会污染对方输入行的形态是**裸按键**（`input: "Enter"`），而它这次被 auto-mode 分类器当场拒了。
- **所以**：`⌛ 等人拍板 → 机长代答` 这条链目前**没有一个既可用又安全的动作**。
  这不等于「链断了」—— 发实质文本这条路是通的（同日实测通过一次），
  但它**只在对方输入行干净时安全**，而机长**看不出输入行干不干净**，除非先 `inspect_session` 看那一屏。
- **当天的实证代价**：44-5 / 44-6 的输入行里各躺着一句来源不明的话，其中一句是
  「跑一次真 daemon 冒烟」—— **会起真进程**。若那次分类器没拦、或有人「换个说法绕过去」，
  缝合出去的就是它。**这一次「被拦」和「没绕」不是流程上的正确，是实质上避免了一次破坏。**
- **修的方向**（未定，别当结论）：给解围一个**只提交对方已有输入**的动作，
  并让它在提交前把那一屏的内容回给调用者看；在那之前，
  **发 nudge 前先 `inspect_session` 确认输入行是空的**，这条是当下唯一的护栏。
- **排序**：排在「方向键通道加不加」之前 —— 那条讨论的是**多一种按法**，
  这条讨论的是**现有的按法会不会替人发出他没写过的指令**。

---

### 🟡 skip 数不能跨环境比 —— 它是「此刻哪些现场数据读得到」的属性

> **2026-09-07 订正**：这条最初写成「『skip 掉回 3』这条基准是假的」。**那个说法被推翻了**，
> 见下面①。基准是真的、可达的。**订正的过程本身留在这儿**，因为推翻它的方式正是本条要讲的东西。

- **发现**: 2026-09-07 · Todo #107/#108 合 main 后按「回共享目录重跑、skip 掉回 3」对账时查出来的。
- **成立的那部分**: skip 数**不是「哪棵树」这么简单**，跨环境直接比会骗人。四个数各有出处，全对得上：

  | 环境 | fixture | skip |
  |---|---|---|
  | 共享目录（fixture 的原产地） | 在 | **3** |
  | 新开的 worktree / CI | 被 `.gitignore:27` 挡掉 | **11** |
  | 发版闸门的 worktree | 脚本显式 `cp -R` 拷进去 | **3** |
  | 上面任一 + 白板此刻读不动 | — | **再 +3** |

  所以 **±8 取决于在哪棵树、有没有把 fixture 拷进去；±3 取决于白板此刻可读性**。

- **① ⚠️ 最初写的「两棵树都没有这份 fixture、谁跑都拿不到 skip=3」是错的 —— 我们查了一个同名但不同的目录。**
  真路径是 `Tests/PendingCrewTests/Fixtures/LEDDriverCrew/whiteboard.json`（58801 字节，共享树里一直都在）。
  我们查的是 `Tests/Fixtures/whiteboard.json` —— **仓库里有两个叫 `Fixtures` 的目录**，
  那个短的装的是 `tui-*.bin` 且进了 git，而被 gitignore 的是长的那个。**两个人都凭直觉挑了短的。**
  - **路径的唯一事实源是 `scripts/make-chat-fixtures.sh:44` 的 `DEST=`**，闸门脚本专门从那行
    `sed` 出来、注释写着「路径知识就只有一份」。**别自己拼路径** —— 仓库里已经有人守住了这条，我们没去用。
  - 复查同名目录的成本是一条命令：`find Tests -type d -name Fixtures`。

- **② 最该记住的一条：账上那条「既有飘红」在缺 fixture 的环境里从未运行。**
  本文件登记的 `CrewChatOpenCostTests.test_打开LED驱动板一次重排在预算内` 就在那 8 条跳过里。
  整整一天，多个人反复说「那两处既有飘红这一趟没撞上」—— 听起来是「它跑了、没红」，
  **实情是在那趟 worktree 上它一次都没跑**。
  这正是本仓库反复吃亏的那一族（`Executed 0` 报 passed、筛掉的红样本不进名单）——
  只不过这次骗的是**我们自己写的账**：一条被登记为「已知会飘红」的测试，在某些环境里
  沉默地退化成「根本不跑」，而汇总行上两者长得一模一样。

- **③ skip 数会自己变（实测 11 ↔ 14）**：那 3 条 `CrewMentionFilterRealWhiteboardTests`
  读的是**现场白板目录**。白板 EPERM 期间它们跳过；白板恢复后同一棵树重跑，18 项里只跳 2 条，
  差值正好是 3。

- **④ 共享目录不可编译时，这条基准连数都产不出来**（2026-09-07 实测）：
  当时 `DaemonStopTests.swift` 已调 `SessionOrchestratorLock.releaseForTesting` 而被测类型上
  还没有这个成员（调用 1 处、定义 0 处），`** TEST FAILED **` 是 **build 阶段**挂的、
  **`Executed` 一行都没有**。**红得没量，和绿得没量一样不构成证据。**

- **能用的替代做法**:
  - 对账盯**逐条名单**，不是那两个数字：`Executed N` + **跳过了哪几条**。
    数字相等而名单不同，是这条基准最常见的骗法。
  - 报「既有飘红没撞上」之前，先确认它**这一趟真的跑了**（在跳过名单里就不算跑）。
  - **跳过名单里出现「本来该盯着某个已知问题的那条」，就等于那个问题这一趟没人看着。**
    今天正是这样：我们以为有三条飘红在被观察，实际其中一条从头到尾没上场。
  - 跨环境比数之前先问：这两趟的**现场数据可读性**一样吗（白板读得动吗、fixture 在不在）。
    不一样就不可比，标注清楚，别混用。

- **⚠️ 这条最值钱的地方是它是自伤**：「跳过和通过在汇总行里长得一模一样」这句话
  **是我们自己写在这本账里的**，而**就在引用它的同一天，我们用同一个错误方式读了
  自己的汇总行**，而且不止一个人、不止说了一次。
  **一条规矩被它的作者违反，比规矩本身更能说明它为什么必要** —— 所以这句留在这儿，不要修饰掉。
  - **订正这条时又栽了第二次**（查错目录），也留着：**第一次是没按自己的规矩办，
    第二次是按了、但量错了对象。** 两次合起来才是完整的教训。

---

### 🔴 一天里栽了四次的同一个错：用「够用的代理量」替代那件事本身

2026-09-07 一轮之内被实测推翻四次。起初看着是四个不同的错，串起来是一条。

| 量的（代理） | 那件事本身 | 怎么破的 |
|---|---|---|
| commit hash 在不在祖先里 | 内容在不在 main | 分支 rebase 过，hash 全换，判成「没落 main」，还差点据此广播「commit 变游离、快被 gc」的警报 |
| `list_sessions` 说「app 未在跑」 | 那个 worker 有没有产出 | 它活着，而且已经合完了 |
| 汇总行的 skip 数 | skip 的**逐条名单** | 把 skip 读成 pass，说「那条既有飘红没撞上」，实情是它没跑 |
| 自己拼的路径 | **定义这条路径的那处**（`DEST=` / 常量 / 配置） | 仓库里有两个同名 `Fixtures`，凭直觉挑了短的那个 |

同一天别人踩的同族：拿过期的测试数当基线、拿「我没收到」当「它没发」、拿 grep 判文件进没进 target、
拿「回执说已回应」当「账本写对了」。

**为什么这类错特别难自己发现**：**代理量在正常情况下全都对** —— hash 通常不变、状态显示通常准、
skip 通常是 0、路径通常只有一条。于是你会越来越信它。**而它出错的那一次，恰好是你最依赖它的那次**
（异常已经发生，你正拿它去判断异常）。所以「以前验证过很多次」不构成信任它的理由，
反而正是它得以长期潜伏的原因。

**怎么办**
- 问「这件事成了吗」，就去量**那件事的产物**：文件在不在、那句话在不在、名单里有没有它。
  别量它的身份（hash）、状态显示、汇总计数。
- **路径别自己拼**：先找谁定义了它。仓库里往往已经有唯一事实源，而且注释里写着别拼。
- **同名的东西先数一遍有几个**（`find -type d -name X`）。挑短的那个是本能。
- **通道故障挡下来的错误不算被防住**：这四次里有一次没扩散成全组警报，
  唯一原因是白板当时正好断连。**一个只在通道坏掉时才没造成后果的错误，等于没有防线。**

### 🟡 `LayoutLoopRegressionTests.testSwiftUIRepeatForeverInAnchoredScrollViewSelfExcites` 会被机器负载判红

- **发现**: 2026-09-07 · Todo #107/#108 合 main 后在干净 worktree 上全量重跑时撞到。
- **它不是普通测试，是那把尺子的「负对照」**：其余几条断言线上那些动画**不**自激
  （`n < 10`）；这一条**故意**搭一个已知会自激的组合（SwiftUI `repeatForever` ＋
  滚动锚点 ＋ `scrollTo`），断言它**确实**自激（`XCTAssertGreaterThan(n, 1000)`）。
  它存在的意义就是「先证明这把尺子会红」。
- **为什么会飘**：`n` 是在一段安静观察期里数到的布局次数。机器忙的时候这段窗口里
  跑不满，`n` 掉到 1000 以下 —— **不是自激没了，是没数够**。这台机器上常年好几个
  `xcodebuild` 并行。
- **实测（不是推的）**：同一棵干净 worktree、同一条测试、隔离重跑 ——
  合并后的 `dce1b01` 上 **红/红/绿**，合并**之前**的 `b4e0ef5` 上 **绿/绿/红**。
  两边同形，**与本次改动无关**；单条整跑约 33 秒。
- **它的失败信息是对的、也是它值钱的地方**：断言语里写着「要么 SwiftUI 修了这个坑，
  要么这条测试的判据失效了，两种都要人来看一眼」。**问题是它没把第三种可能算进去
  ——机器太忙。** 于是它每飘红一次，就是在喊「尺子可能坏了」，而实际只是负载。
- **处置建议（没做，交给 #443）**：让它对负载免疫，而不是抬阈值 —— 抬阈值会让它
  在真的「SwiftUI 修了这个坑」时也不报。可选方向：观察期改成**按布局次数收敛**
  而不是按墙上时间，或在断言里带上实际观察时长、区分「没自激」与「没观察够」。
- **同族**: 与 `CrewChatOpenCostTests`（性能预算）、`CrewLocalImageCacheTests`
  （NSCache 内存压力）是同一族 —— **拿一个跟机器负载相关的量当断言**。
  这已经是这台机器上第三条按构造就会飘的测试了，隔三差五随机把 main 判红。
- **⚠️ 附带一条别的教训：判「这条记录到底落没落」，两头都能骗人**
  （来源：机长 15-1，2026-09-07，核这条时差点栽的一跤）。
  - **按 hash 判会假否定**：`git merge-base --is-ancestor <hash> HEAD` 说 ✗ —— 因为
    rebase 会把 hash 换掉，同样的内容换了个号。
  - **改按内容判、但查得太粗会假肯定**：`grep -c LayoutLoopRegression` → **4 处命中**，
    差点结论「已经在了，不用落」。那 4 处是**同一个测试类里的另一条**（LazyList 变体），
    而这一条是 AnchoredScrollView 变体 —— 本文档自己还写着「这两条方向相反，更别合并」。
    精确重查 `grep -c AnchoredScrollView` → **0**，确实没落。
  - **可执行**：判「已完成」没有一把天然安全的尺子，只有一个前提 ——
    **说得出你在找的那一个长什么样**（那个只属于它的字串），然后拿它去查。
    说不出来，就说明你还不知道自己在找什么，两种查法都会给你一个看起来很像的答案。

**观测记录 —— 每次「真跑过」就追加一行，绿也要记。**

> **为什么绿也要记**：只记红，这张表就永远只能证明它飘 —— 那正是「已知飘红」这个标签
> **自我实现**的机制。贴上标签的那一刻，这条测试在两个方向上都不再携带信息：红了「它本来就飘」，
> 绿了「一趟绿不算数」。**它就此退出了检测器的行列，只是没人宣布过。**
>
> **必须填条件那两栏**：这是负载敏感断言，**没有边界条件的读数不构成证据**。
>
> **退出条件（摘掉「飘红」标签的门槛）**：连续 **3** 次在**有负载**环境
> （同时 ≥4 个 `xcodebuild` 在跑）下通过 → 重新评估这个标签。
> 取 3 的依据：账上那次红是在 19 个并发下发生的，1 次绿分不清「运气」和「趋势」，
> 3 次是这个跑动频率下最小的可判样本。**这个数可以改，但必须有一个数**
> ——否则这个标签永远摘不掉。

| 日期 | 环境 | 结果 | 耗时 | 同时在跑的 xcodebuild |
|---|---|---|---|---|
| 2026-09-07 早 | 共享树 `e4033dc`，机器较空 | passed | 用例耗时 5.445s；**被测指标（布局次数 n）未记** | **未量**（当时没数——第一行就缺这一栏，正好说明这张表为什么需要它） |

> **⚠️ 这一行有两栏是残的，故意留着**：这条断言量的是**固定窗口内的布局次数 n**，
> 而我记的是 XCTest 的用例耗时 —— **不是同一个量**。下一个跑它的人请把 `n` 记下来
> （通过时也记），否则这张表攒不出「负载 → n 变小 → 跌破下界」那条曲线。

### 🟡 并行派活按「代码段」划红线 —— 盲区是**生成物**，两边段不重叠也照样撞

- **发现**: 2026-09-07 · Todo #107/#108 与「判活看产出」两个 worker 同时开工时**提前**发现，
  不是事后复盘。机长按段划红线（一个只碰 `McpServer.swift` 的 `plan_add`/`plan_update`，
  另一个只碰 `list_sessions`），**源码这一层的确没有重叠**。
- **撞的地方**: 两边各自新增了 Swift 文件，于是各自跑了 `xcodegen generate` ——
  **`PendingCrew.xcodeproj/project.pbxproj` 两边都改，而且是从不同基线生成的。**
  先合的没事，后合的必然在 pbxproj 上冲突，而那是**生成物，不能手工合文本**
  （`--ours` / `--theirs` 都会丢掉另一边的文件引用，而且**编译器不会报错**——
  丢的那个文件只是不进 target，测试照跑，`Executed N` 少几条没人会注意）。
- **另半边：也不能「还原」**（来源：机组群聊体验 1-1，2026-09-07，自己踩出来的）。
  它为了「只提交自己的东西」，把共享树里的 `project.pbxproj` `git checkout` 回 HEAD，
  **当场打断了别人的编译**（`Sources/Mac/Services/SessionDaemonMain.swift:238: cannot find 'DaemonGracefulShutdown'`）。
  pbxproj **不是任何人的私有文件，是全员共用的桥** —— 还原它等于替别人做决定。
- **处置（已定为规矩）**: 合并冲突时先定顺序，**后合的人不解冲突**，合完直接重跑
  `xcodegen generate`，把重生成的 pbxproj 一并提交。合起来的完整规矩是：
  > **共享工作区里的生成物，既不能挑边合（`--ours`/`--theirs` 会静默丢文件、
  > 只表现为 `Executed N` 少几条），也不能还原（那是全员共用的桥，不是你的私有文件）。
  > 唯一安全的动作是重新生成。**
- **⚠️ 一条听起来能防住、实际防不住的推论（本条最该记住的部分）**：
  「加 Swift 文件的活一律派进独立 worktree，regen 只影响自己那份，就没这问题」——
  **错。** 撞车的这两个 worker **本来就都是 `isolation: true`**，照样撞了。
  两层要分开说，别合成一条：
  - **工作区层面**：改生成物的活确实该隔离 —— 否则会踩到别人正在用的桥（1-1 那个坑）。
  - **合并层面**：**隔离不能消除生成物冲突。** 两条分支各自从不同基线 regen 同一个
    生成物，冲突发生在 merge，跟隔离与否无关。唯一解法就是上面那条：
    **定合并顺序 + 后合者重新生成。**

  把它写成「隔离就没这问题」，下一个人会照做、然后在合并时撞上同一堵墙，
  **而且会因为「我已经隔离了」更晚才想到真正的原因**。
  （这条推论是本条目第一版真写过的，被机长当场纠正 —— 留在这儿当反面例子：
  它正是这一簇要治的病的形状，一个听起来能防住、实际防不住的东西。）
- **⚠️ 而且它常常不冲突** —— 实测 2026-09-07：第二个 worker 合进来时 pbxproj
  **`Auto-merging` 直接过了，一个冲突都没有**。生成物的自动合并**比冲突更危险**：
  冲突会叫你，自动合不会，而合出来的那份是不是两边文件都在，没有任何东西会告诉你。
  **所以「后合者重新生成」不是冲突时的应急手段，是无条件动作** —— 不管有没有冲突。
- **⚠️⚠️ 但「重新生成」这个动作本身有个坑，我当场踩了（2026-09-07，实测）**：
  `xcodegen` 扫的是**磁盘目录**，不是 git 索引 —— **在共享树里 regen 会把别人
  正在写、还没提交的文件一起烤进 pbxproj。** 我合完在共享树里 regen，把第三条线
  刚创建、还是 untracked 的 `PendingCrewLaunchAgent.swift` 写进了提交出去的
  pbxproj。**后果是反过来的**：任何新 worktree / 干净 clone 上那个文件都不存在，
  pbxproj 却引用它 → 直接编不过。
  **而且它伪装成一个正确的发现**：regen 后的 diff 比自动合出来的多 6 行，看起来
  正像「自动合并静默丢了文件」——我第一时间就是这么判的，还写进了合并说明。
  真相相反：**自动合出来的那份是对的，是我 regen 时把不该有的东西加了进去。**
  分清只需要一件事：**在一棵干净的树里 regen**（`git worktree add --detach <合并后的 commit>`
  再 regen），拿它当基准去 diff。这次干净树的产物与自动合并的那份**一字不差**。
- **所以规矩要连场地一起说**：**后合者的重新生成必须在干净树里做**，
  把产物拷回来；在还有别人未提交改动的共享树里 regen，等于替别人提交了半截东西。
- **为什么值得单独记一条**: 「按段划红线」这套办法本身是对的，它的**假设**是
  「两个 worker 的产出面 = 他们改的源码段」。**这个假设在任何有生成物的仓库里都不成立**
  ——pbxproj 只是最显眼的一个，同族还有：任何 `codegen` 产物、lockfile、
  重新导出的索引/清单文件。**划红线时要连生成物一起划**，并在派活那一拍就说清
  「谁先合、后合的怎么收尾」。
- **同族账**: 「红线按段不按文件」那条讲的是**方案变更**会让名单失效；这条讲的是
  **名单从一开始就漏了一维**（源码之外的产出面）。两条要一起读。

**第四张脸（2026-09-07 机长 15-1 亲历）：`git add` 会捞走别人未提交的改动。**

前三张都是**生成物**上的（挑边合 → 静默丢文件；`git checkout` 还原 → 拆掉别人正在用的桥；
在共享树 `xcodegen` → 把别人还没提交的文件烤进 pbxproj）。第四张不限于生成物，**任何文件都算**：

我改完 `docs/tech-debt.md`、`git add` 之后要提交，git 回 **`no changes added to commit`** ——
因为另一条线的 `27e62de`（一个 daemon 修复）**已经把我那 62 行一起提交了**。

**它最阴的地方是两边都不会看到错误**：
- 我这边只看到「没有改动可提交」，长得像**我的改动丢了**；
- 它那边一切正常，测试也照过（内容本来就是对的）；
- 而 `git log -- docs/tech-debt.md` 从此把这段改动的出处指向一个**毫无关系的 commit**。
  半年后有人问「这两张观测表是谁加的、为什么」，git 会给他一个自信的错答案。

**怎么办**
- **改完立刻提交**，别让自己的改动在共享树里过夜 —— 暴露窗口 = 你从改完到提交之间的时间。
- 看到 `no changes added to commit` 而你确信改过，**先 `git log -1 -- <那个文件>`**，
  多半不是丢了，是被别人带走了。
- 反过来：**在共享树 `git add` 之前，先 `git status` 看清这个文件是不是只有你在改**。
  按文件名逐个 add 挡不住这条 —— 你 add 的那个文件里可能同时有别人的行。
- **不必回滚重提**：内容对、位置对时，为一个 commit 归属去改写别人的历史不划算。
  记一笔比改历史便宜。


### 🔴 还有 3 处 `open()` 没有 `O_CLOEXEC` —— 锁会被 agent 子进程继承走

- **发现**: 2026-09-07 · P5b 做停用入口时在真机上撞到
- **实测**: `lsof ~/Library/Application Support/PendingCrew/orchestrator.lock` 列出
  daemon `77461` **和它的 11 个 claude 子进程**，全部 `3u` —— 同一个打开文件描述。
  病根：`open()` 的 fd 默认跨 `exec` 继承，而 agent session 走 SwiftTerm 的
  `forkpty`（裸 fork+exec，不关 fd）。Foundation 的 `Process` 在 Darwin 上默认
  CLOEXEC-all，所以**只看 `Process` 那条路会以为没事**。
- **为什么严重**: flock 挂在打开文件描述上。daemon 崩了而 session 还活着时，
  锁被那些孤儿子进程继续持有 → 下一个 daemon 取锁读到一个**已死 pid 的持有者** →
  判成「已经有一个 daemon 在跑」→ **安静退出 0**。**谁也起不来，而且没有任何报错**，
  直到最后一个子进程死掉。这正是 `SessionOrchestratorLock` 类型注释里说「pid 文件
  才会有、flock 不会有」的那个死结 —— **那句保证一直是被这个缺失的标志位无声否定的。**
- **已修**: `SessionOrchestratorLock` 两处 `open` 都加了 `O_CLOEXEC`，
  `testTheLockFdIsNotInheritedAcrossExec` 直接读 `FD_CLOEXEC` 钉着（变异自证过）。
- **本条登记的是剩下 3 处**（都不是我这条线的，没动）：
  - `Sources/Stores/MultiProcessJSONStore.swift:34` — `withFileLock` 的锁文件。
    **这处后果最重**：白板 / 人类 Todo / 审批账全走它，而它用的是**阻塞** `flock(LOCK_EX)`。
    持锁进程被杀在写的半路上时，锁活在孤儿子进程里，**后续所有写会永远阻塞**。
  - `Sources/Mcp/WhiteboardCursor.swift:181` — 同一形状的游标锁。
  - `Sources/Stores/LocalWhiteboardStore.swift:117` — 目录监听的 `O_EVTONLY`，
    后果轻（只是 fd 泄漏到子进程），但同一批改掉最省事。
- **改法**: 三处各加一个 `| O_CLOEXEC`，外加一条读 `FD_CLOEXEC` 的守卫测试。
  **别拿「起个 `Process` 子进程看看」当判据** —— 那条路两个方向都绿（我第一版就这么
  写的，去掉 `O_CLOEXEC` 之后照样过）。

### 🟡 `try?` / 非抛版 `write` —— 写侧的三种穿法，只堵住了会崩的那一种

- **发现**: 2026-09-06 · 父 crew 派的 P0（9/5 15:50 daemon 被未捕获 NSException 打死，5 个 crew 同时掉 session）
- **已修的那一处**: `CodexAppServerConnection.swift` 的 `writeLine` —— 签名写着 `throws`、6 个调用点全写了 `try`，
  函数体却调 ObjC 的 `writeData:`。**唯一真会发生的错误（EPIPE）恰恰是那些 `try` 捕不到的那个。**
  换成 `CodexPipeWrite.line`（会抛的 `write(contentsOf:)` + 自带 SIGPIPE arming），3 条测试自证。
- **留着没改的（本条登记的就是这些）**:
  - `Sources/Mac/LocalRunner/SessionDaemonHost.swift:111`（`SessionDaemonLog.write`）— `try? handle.write(contentsOf: data)`
  - `Sources/Mac/LocalRunner/GitWorktreeService.swift:185` — 同样形状
  两处都写**普通文件**，不会 EPIPE，所以不会崩；但 `try?` 会把真实写失败（盘满、权限、
  卷被卸载）**静默吃掉**。daemon 日志写不进去这件事本身是无声的 —— 而那份日志正是下次
  出事时唯一的现场。**记账不改：这是另一件事，改它要先想清楚「日志写不进去时往哪报」，
  而那个去向本身就是个设计问题（往日志报日志写不进去，是个环）。**
  - 5 处 `FileHandle.standardError.write`（非抛版，往 stderr 写）：
    `Sources/Mac/LocalRunner/SessionLaunchOptions.swift:128`、`Sources/Mac/Services/SessionDaemonStatusMain.swift:17`、
    `Sources/Mac/Services/SessionDaemonAttachMain.swift:22`、`Sources/Mac/Services/SessionDaemonAttachMain.swift:29`、
    `Sources/Mac/Services/SessionDaemonMain.swift:44`。**5 处 4 个文件** —— 派活的 brief 里写的是 3 处，
    实测是 5 处（报数时把名字列全再数一遍，撞上过一次就知道值）。
    stderr 的读端在 CLI 场景**通常**是终端，不会断 —— 但「通常」不等于「一定」：
    `PendingCrew --daemon-status 2>&1 | head` 那根管子的读端一走就断。
    **2026-09-07 P5b 补了正确的那一种**：`StandardErrorText.write`（`DaemonStop.swift`），
    用 `fputs` —— 写失败只返回 EOF，不会把进程带走。新写的 `--daemon-stop` 走它。
    **这 5 处该迁过去，但迁移不在那一笔里做**，登记在这里。
    （别再新增第 6 处 `FileHandle.standardError.write`：正确的那一种已经有了。）
- **顺带一条必须写明的边界**: `CodexPipeWrite` 里那次 `signal(SIGPIPE, SIG_IGN)` 是**进程级**的。
  它只在 `CodexPipeWrite.line` 被调用过之后生效，而上面那 5 个 stderr 点所在的 CLI 入口
  （`--daemon-status` / `--daemon-attach`）从不调它 —— 所以**对它们没有任何影响**。
  app / daemon 进程里 AppKit 本来就忽略 SIGPIPE，也没变化。
  写下来是因为「进程级副作用」这种东西必须有人说清它够到哪、够不到哪，否则下一个人
  只能靠猜。


### ✅ session 协议的收包路径假设「一次投递 == 正好一整帧」（**P4 `f34d7c9` 已还，2026-08-29**）
- **发现**: 2026-08-29 · 父 crew Todo #44（Fly 远程主机接入复核，`docs/internal/2026-08-29-fly-remote-host-review.md` §2 A-2/A-3）
- **已还**: server 每条连接与 client 各持一个 `SessionFrameDecoder`，endpoint 面向
  `SessionMessageLink` 的可靠有序字节流语义；新增 transport 替身把第一帧切成半包、再与第二帧
  粘在同一次投递里。旧代码实跑 6 tests / 4 failures，修后专项 32 / 0；合最新 main 后全量
  1766 tests / 3 个登记 skip / 0 failures，iOS Simulator build 通过。
- **位置**: `Sources/Mac/LocalRunner/RemoteSessionBackend.swift:567-568`（server 侧 `receive`）、
  `:855-859`（client 侧 `receive`）、`Sources/Mac/LocalRunner/SessionProtocol.swift:469` / `:486` →
  `:503-509` `exactlyOneFrame(_:)`。
- **问题**: 两个 endpoint 收到 `Data` 后都直接 `codec.decodeApp/decodeDaemon`，而它们经 `exactlyOneFrame`
  要求这一次投递**恰好解出一帧**；不满足就 `throw`，调用点是 `guard let … = try? … else { return }`。
  于是**半帧到达 → 丢；两帧粘在一起 → 两帧都丢**。不断连、不报错、不落日志。
  正确的增量缓冲 `SessionFrameDecoder`（带 `buffer`、能处理半帧，`Sources/Mac/LocalRunner/SessionProtocol.swift:81-107`）已经写好了，
  **只是没接到 endpoint 的收包路径上**。
- **为什么今天照不出来**: `InProcessTransport.sendFromApp/sendFromDaemon` 把整个 `Data` 原样交给对端回调
  （`Sources/Mac/LocalRunner/InProcessTransport.swift:22-30`），投递边界恒等于帧边界。**现有测试全部跑在这条传输上，所以这条永远是绿的。**
  UDS 上偶尔踩；WAN + TLS（Fly 远程主机、手机遥控）上必然拆包粘包。
- **连带一条**: 两个 endpoint 的 init 签名是 `init(transport: InProcessTransport, …)`（`:479` / `:763`），
  不是同文件里已经定义好的 `SessionTransport` 协议（`Sources/Mac/LocalRunner/InProcessTransport.swift:6`）—— 换传输必须改这两个 init。
- **该怎么还**: 归 P4（真进程分家）的验收，**不要平行改**（那是设计 §6 警告的双头）。两步：
  ① 每条连接各持一个 `SessionFrameDecoder`，`receive` 改成「喂字节 → 拿 0..n 帧 → 逐帧处理」；
  ② endpoint 面向 `SessionTransport` 而非具体类。
  验收要求**先证明尺子会红**：写一个按任意字节边界切分/合并投递的传输替身，跑当前代码必须红，接上增量解码后转绿。
- **已还（2026-09-04 核实，随 P4 合入 main `a8f4597`）**：两个 endpoint 各自持一个
  `SessionFrameDecoder`（server 侧在 `Connection` 上，`Sources/Mac/LocalRunner/SessionProtocolEndpoints.swift:21`；
  client 侧 `:541`，重连时 `:593` 重置），收包路径改成「喂字节 → 拿 0..n 帧 → 逐帧处理」。
  传输面向 `SessionMessageLink`（`Sources/Mac/LocalRunner/UnixSocketTransport.swift:15`）而非具体类，
  今天有四个实现：`InProcessSessionLink` / `UnixSocketTransport` / 测试替身
  `ByteStreamLink`（**按任意字节边界切分与粘包**）/ `BackpressureLink`。
  尺子也照要求写了：`SessionProtocolOverSocketTests.test_server接受任意切分与粘包的可靠字节流`
  与 `test_client接受任意切分与粘包的可靠字节流`。
  **没验到的**：真 WAN + TLS 上没跑过（异机传输是 P6，见
  `docs/internal/2026-09-04-cross-machine-transport-scope.md`，尚未实现）。

### 🟡 唤醒投递闸和它的重试路共读同一个瞬时布尔 —— 只改一边会打架

- **发现**: 2026-09-05 · 修「压缩上下文被误判卡死」（`63f39ac`）时顺手看到，**没修**
- **现状**: `CrewSessionRunner.deliverOrDeferWake` 用 `run.activityIsWorking`
  （「最近 1s 内有 PTY 输出」）判目标闲不闲、决定立刻投还是排队；
  `scheduleDeferredWakeRetry` 0.5s 后**再读同一个布尔**决定要不要补投。
  claude 压缩上下文/长思考时这个布尔读到 false，于是照投不误。
- **为什么这一笔没顺手改**: 注入进一个忙碌的 claude 是安全的（TUI 自带输入排队，
  2026-09-05 人类那条消息就是这么送到的），所以它不是**故障**，只是不精确。而闸
  和重试路读的是同一个信号，只给闸换上更准的判据（`63f39ac` 新加的忙碌指示）、
  重试路还读老布尔，两者就会打架：闸判「忙，排队」，0.5s 后重试路判「闲，投」——
  排队等于白排。要改就得两处一起换，那是另一笔。
- **代价**: 目前只有「投得早了一点」，没有可观测的坏结果。真要动的时候记得：
  **先看这两处是不是还共读一个信号**，别只改看得见的那一处。

### 🔴 数据根有**三个**来源，而纪律只承认一个 —— 附件那处直接绕过整套隔离

- **发现**: 2026-09-05 · P5a 发包前的冒烟审计（父机长起头，我与 worker 逐条钉死）
- **纪律写的是什么**: `Sources/Stores/PendingCrewDataRoot.swift:23-27` 三条纪律，第一条
  是「**所有人从这里拿路径，不许再有第二个 `getenv`**」。文件头 `:14-21` 还写着它
  「不是配置项，是**能不能验**的前提」—— 因为这台机器上起 daemon 冒烟只有靠它才不双头
  （27 秒那次事故改坏了 `quota.json` / `models.json` / `crew-sessions.json`）。
- **实际有三个来源**:
  1. **env `PENDINGCREW_DATA_DIR`** —— daemon 走这条（`ps eww` 量到）。
  2. **argv `--dir`** —— helper 走这条。`Sources/Mcp/McpHelperMain.swift:24` 取参数，
     六个 store 全部 `directory: dir`。**注意 env 到不了 helper**：
     `Sources/Mac/LocalRunner/LocalCodingAgentSpec.swift:67-77` 的透传白名单只有 8 个键，`:83-86` 专门封死
     `PENDINGCREW_` 前缀（secret 卫生，本身是对的）。所以隔离能成立**全靠这第二条通道**，
     而纪律里没有它。
  3. **静态默认** —— `Sources/Mcp/McpServer.swift:79`
     `attachmentRoot ?? CrewChatAttachmentStore.defaultDirectory`，而
     `Sources/Stores/CrewChatAttachmentStore.swift:16` 的 `defaultDirectory` = `PendingCrewDataRoot.subdirectory("attachments")`
     = **真数据根**。`McpHelperMain` 从头到尾**没传过 `attachmentRoot`**（grep 零命中）。
     → **隔离环境里的 agent 只要 `post_to_crew(attachments:)`，附件就写进真数据根。**
- **为什么它今天没咬人**: 纯属运气 —— 冒烟的 brief 都是纯文本，没有一次带附件。
  发包那趟我们把「不许带 attachments」**写进了任务书正文**（不是靠「判据里没有它」兜）。
- **同族、今天没触发的第四处**: `Sources/Mcp/McpHelperMain.swift:70` / `:77` 的 hook 分支写的是
  `dir ?? LocalWhiteboardStore.defaultDirectory` —— **`--dir` 一旦没传就静默落回真数据根**。
  现在每条 `ps` 都带着 `--dir`，所以不是问题;但它跟 `:79` 是同一种「兜底指向真目录」的写法。
- **修**: helper 补传 `attachmentRoot`（连同 `:70`/`:77` 那两处兜底一起收口）。**更值得做的是
  让错的写法表达不出来**：三处 `?? …defaultDirectory` 都是「忘了传就悄悄用真目录」，
  而这正是隔离最不能容忍的默认方向 —— 隔离场景下应当**没有默认值，不传就编不过或直接失败**。
- **方法论（这条比缺陷本身值钱）**: 审这件事时我先报了「六个 store 全部注入 `dir`，一个都没走
  默认根」—— **错的**。我列的是**注入清单**，而风险在**使用面**，`attachmentRoot` 恰恰是
  「没被注入」的那个，所以它不在我的清单里。**清单是「给进去的」，风险在「用到的」，
  两者的差集就是漏洞住的地方。**

### 🔴 所有在跑 session 的 PTY 输出都要过主线程 —— 界面代价随派活数线性增长
- **发现**: 2026-08-19 · `fix/ui-jank-pty-scan`（Todo #59 界面卡顿排查）
- **位置**: `Sources/Mac/LocalRunner/AgentTerminalSession.swift` 的 `dataReceived` 回调（`ActivityTerminalView.dataReceived(slice:)` → `MainActor.assumeIsolated { … }`）。
- **问题**: session 本体是 `LocalProcessTerminalView`（SwiftTerm 的 `NSView` 子类），PTY 每一批输出都在**主线程**交付，**不管这个 session 在不在前台、用户有没有在看它**。挂在这条回调上的旁路工作（打字指纹、健康扫描、菜单检测、启动参数回显、切档回显）因此全部是主线程同步工作，且**按在跑的 session 数线性叠加**。
- **证据**: `docs/internal/2026-08-19-ui-jank-profile.md`。sample 实测：单个忙碌 session 的 `dataReceived` 子树占主线程 **7.9%**；机长常态派 3～5 个 → 24%～40%。
- **本次做了什么**: 把这条回调上**每一段**旁路工作的单价打下去了（squeeze O(n²)→O(n)、健康短语改字节匹配、打字指纹单趟折叠），合计把单 session 的稳态占比压掉一个数量级以上，并消掉了「每起一个 session 主线程被占死 45 秒」那条。
- **没做也不该在这条分支上做的**: **结构本身没变** —— 代价仍然随 session 数线性涨，仍然全在主线程，仍然与「用户在看哪个终端」无关。真正的解法是把 session 搬进常驻后台进程（机长 2026-08-19 已在单独排期，与「前后端分离」同一刀），app 退化成看的那个窗口。**这条不要顺手在功能分支里动。**
- **修后复采（2026-08-19）**: 结构没变，但单价打下去之后实测 **10 个 session（6 个在忙）合计只占主线程 1.51%**（≈0.25%/忙碌 session）。
  也就是说这条 🔴 现在**不再是当前的卡顿来源**，它仍然是 🔴 是因为「代价随 session 数线性涨、且与用户在看哪个终端无关」这条结构事实没变 ——
  派到几十个 session 时它会重新变成主导项。见 `docs/internal/2026-08-19-ui-jank-profile.md`「现场复采」。
- **中间态的可选缓解**（如果地基活拖久了）: 后台 session 的旁路扫描改成攒批 / 挪出主线程；或把 scrollback 与扫描窗按「是否前台」分档。都属于治标，记在这里免得被当成已解决。

### ✅ 远端 / 登录整层的「安静死代码」（**随 #63 第二期删除，2026-08-26**）

> 跨端遥控，端掉。以后前后端解耦时重新做

上面这句是人类原话，一个字没改。**这不是清理垃圾，是有意移除、将来在新架构下
重建**——写在这里是为了将来有人翻到这段历史时，不会以为这块是被谁偷偷删掉的。

- **原问题（2026-08-25 · #63 第一期落地时记）**: 第一期删掉了**取得凭据的所有入口**
  （Auth 三件套 / Supabase 栈 / 两个登录页 / 扫码页 / 侧栏登录入口 / RootView 的登录
  分支），凭据层本身却删不动（被 `Sources/Remote/`、`CrewRelayAgent` 那批文件顶着）。
  于是 `AppModel.isAuthenticated` **恒 false**、`loggedAPIClient()` 恒抛、`imageAuth`
  恒 nil，所有调用点都是 `guard … else { return }` / `try?` 早退 —— 编译器不报、测试
  不红、看代码也看不出来。
- **第二期做了什么**: 从调用点往被调用方走，分九笔删干净：「服务端 session」面板 →
  crew 详情页的「接入 PendingBot」节 → serverLink 那条链（`Sources/Remote/` 三个文件
  + `SessionPermissionRelay` + `CrewMailboxWaker`）→ `CrewRelayAgent` 与两个纯逻辑
  → 本地两个 store 上的 relay 残留 → edge 交互卡 → `attachmentIds` 通道 →
  `EdgeBackend` + `CrewRealtimeClient` → `AppModel` 凭据层 + `PendingCrewAPI` +
  Keychain / 家族凭据。**没有用 `#if false` / 注释掉 / 空 stub**，全仓 `#if false` 零命中。

- **名单的修正**（比删除本身更值钱的那部分，逐条列）:
  - **`CrewRelayAgent` 不是「部分死」，是整个死。** 上面那张表把它记成「被
    `LocalWhiteboardStore` / `LocalCrewStore` / `SessionHost` / `CrewLocalTodoLanding` /
    `CrewRelaySyncLogic` 五个本地活路径引用着」——实测**只有 `SessionHost` 那一处是真
    代码引用**（`let relay: CrewRelayAgent`），其余四处全是注释里提到它的名字。
  - **`CrewMailboxWakeLogic` 不该跟着 waker 走。** 它一半是 edge mailbox 决策
    （`decide` / `renderInjection`，跟着删），另一半是**唤醒投递回执**
    （`receiptVerdict` / `wakeFailureAlert`）—— 跟 edge 无关，本地 @ 直投在用，留下。
  - **`senderDisplayName` 有两个，同名不同物。** 存储层
    `LocalWhiteboardMessage.senderDisplayName` 唯一写入方是 `appendRelayMessage`，
    死了；线上模型 `CrewWhiteboardEntry.senderDisplayName` **是活的** ——
    `LocalBackend` 用本地 `senderName` 主动合成它，中栏靠它把本地 session 的消息显示
    成「机长」而不是兜底「会话」，`CrewChatAdapterTests` 第 6 例专门钉着。只删了前者。
  - **`relayRemoteId` 不是孤儿。** 它有活读者 `CrewLocalMentionWakeLogic:62`
    （#554 断链修复的规则 3）。删是删了，但那是**一条真修过的 bug 的防线**，见下面
    单独一条。
  - **`CrewChatView.swift` 的 `loggedAPIClient()` 是 3 处不是 2 处**（表上写 2）。
  - **表上完全没列、但引用了这批符号的**：`CrewModels.swift`、`ModelCatalogEntry.swift`、
    `LocalRunnerPlaceholder.swift`、`CrewRelayHubLogic.swift`、`CrewMailboxWakeLogic.swift`、
    `CrewRelaySyncLogic.swift`、`LocalDataReset.swift`、`CrewSettingsView.swift`、
    `CrewRealtimeClientTests.swift`、`CrewSummary.swift` / `CrewRootLineage.swift` /
    `CrewListView.swift`（EdgeBackend 注释）、`LocalCrewStore` 上的四个 relay 持久化字段。
  - **交互卡整套在本地路径上一次都没渲染过**：`LocalBackend.listCrewWhiteboard` 把
    `payload.kind` 写死 nil，而 `isInteraction` 就是 `payload?.kind == "interaction"`。
    （**注意本仓有两套「待审批」**：`LocalApprovalStore` 那套是本地权限审批，活的，
    此刻磁盘上就有真实数据，一个字没碰。）

### 🟡 #554「远端人类的 @ 也唤醒」那条防线随 relay 一起没了 —— 重建时要一起重建
- **发现**: 2026-08-26 · Todo #63 第二期
- **原来是什么**: `CrewLocalMentionWakeLogic.pending` 的**规则 3**：`relayRemoteId != nil`
  的 **user** 条目（远端人类经 relay 落进本地白板的 @）也收。理由是 composer 直投只
  覆盖本机人类，远端 iOS 用户 `@session` 落到 Mac 白板后**没有任何投递者**把它转成注入，
  session 就此断链收不到 —— 那是 #554 真修过的一个 bug。
- **现在为什么没了**: 判据 `relayRemoteId` 随 relay 整层删除，条件恒 false。规则、它的
  两个用例、以及 `LocalWhiteboardMessage.relayRemoteId` 字段一起去掉。
- **该怎么还**: **前后端解耦重建 relay 那一刀，必须把这条一起重建。** 判据换成新架构里
  「这条是从远端搬进来的」的等价标记；不重建的话，远端人类的 @ 会重新变成断链，而且
  症状和 #554 当年一模一样（没有任何报错，就是收不到）。

### 🟢 `Sources/Models/ModelCatalogEntry.swift` + `PendingCrewBackend.listModels()` 已无消费者 —— 但**不是 #63 造成的**
- **发现**: 2026-08-26 · Todo #63 第二期（零残留自查时撞见）
- **问题**: `listModels()` 全仓没有任何调用方；`ModelCatalogEntry` 只被 `listModels()`
  的签名引用。新建 session 页早就改读本机实探的 `ModelCatalogCenter` /
  `AgentModelCatalog`（models.json，形状不同）。
- **为什么这一期没动**: 它**在 #63 之前就已经是死的**，不是这一刀造成的孤儿，也不属于
  遥控 / 登录层。按「清单外的不自己扩」留着，只在类型注释里写清楚。
- **该怎么还**: 确认 `ModelCatalogCenter` 那条路是唯一供数方之后，把 protocol 上的
  `listModels()`、`LocalBackend` 的空实现、以及 `ModelCatalogEntry.swift` 一起删。

### 🟢 两个名字在这一刀之后名不副实 —— `CrewMailboxWakeLogic` / `CrewRemoteImage`
- **发现**: 2026-08-26 · Todo #63 第二期
- **问题**: `CrewMailboxWakeLogic` 现在只剩「唤醒投递回执」那半（edge mailbox 决策已删），
  名字里的 mailbox 不再指任何东西；`CrewRemoteImage` 现在只从 `file://` 读本地图，
  Remote 也不再指任何东西。
- **为什么没改名**: 两个都是别处的构造点（前者被 `CrewSessionRunner` 调、后者被
  `ServerImage` / `BubbleView` 构造），改名会把这一刀的 diff 摊进不相干的文件。
  两处都在类型注释里写了「名字是历史」。
- **该怎么还**: 顺手改名的时候一起改（`CrewMailboxWakeLogic` → 回执判定；
  `CrewRemoteImage` → 本地附件图），不值得单开一笔。

### 🟢 `CrewSummary.rootCrewTitles` 与 `CrewRootLineage` 的服务端回退分支现在恒空
- **发现**: 2026-08-26 · Todo #63 第二期
- **问题**: `rootCrewTitles` 原本是服务端算好下发的根 crew 血缘，给看不到本地 DAG 的
  iPad/iPhone 用。云端整层删掉后它恒空，`CrewRootLineage.rootTitles` 里「本地算不出
  才用服务端这份」的回退分支因此不再会被走到。
- **为什么留着**: 判定本身是对的，重建前后端时第二个来源会重新出现在这个位置；
  而且删字段要动 `CrewSummary` 的 Codable 与它的一批测试，收益为零。已在注释里写明。


### ✅ 登录态 session 的信箱唤醒与审批中继从未接通（**随 #63 第二期删除，2026-08-26**）
- **发现**: 2026-08-19 · 前后端分离 P0（所有权归拢）
- **位置**: `Sources/Mac/Views/CrewSessionWindowView.swift` 手动起 session 那条路里的 `let serverLink: CrewSessionServerLink? = nil`；实现在 `Sources/Mac/Services/CrewSessionRunner.swift` 的 `ensureMailboxWaker` / `ensurePermissionRelay`。
- **问题**: 这两个服务原本由视图在 run 起好后接线，两个调用点**都在死路上** —— 一条被上面那个写死的 `nil` 挡着，另一条在被 `edgeQueueBindingReady == false` 关着的 auto-claim 死循环里。也就是说它们**一次都没被调用过**。调研清单（`docs/internal/2026-08-19-backend-split-inventory.md` A19/A20）当时的判断是「右栏没打开过的 session 才没接」，实际比这更糟：**所有 session 都没接**。
- **症状**: 登录态下 edge 信箱的定向投递不会唤醒本机 session；远端 viewer 的审批镜像（#204 permission over WS）不生效 —— 都是静默不工作，没有任何报错。
- **根因**: edge session 通道（接合 v2 block 3，本地 crew ↔ edge 行的绑定）没开，所以 `serverLink` 一直是 nil。不是这两个服务本身有问题。
- **P0 做了什么**: 只删掉视图侧那段永不执行的接线（连同 auto-claim 死循环），**实现原样留在 runner 上并加了注释说明当前无调用点**。P0 的约束是行为零变化，真接上属于行为变化，不在本阶段做。
- **解法归属（历史）**: P4（编排整体搬进 daemon，届时由 runner 侧统一接线，别再从视图接）或云端那条轴（先把 edge session 通道打开）。
- **为什么结掉（2026-08-26 · Todo #63 第二期）**: 这两个服务本身删掉了 —— `CrewMailboxWaker` / `SessionPermissionRelay` / `SessionProxyClient` / `CrewSessionServerLink` 连同 `ensureMailboxWaker` / `ensurePermissionRelay` 两个无调用点的入口一起走。人类原话「跨端遥控，端掉。以后前后端解耦时重新做」——**这条不是修好了，是连同它描述的东西一起没了**。
- **重建时要注意的**: 当年的病根不是这两个服务有问题，是**接线接在视图上**（`CrewSessionWindowView` 起 run 时接）。重建时按 P0 的结论从 runner / 常驻编排侧接，别再从视图接。

### 🟡 点名唤醒器把「读增量」当成廉价操作 —— 每个目录 tick 全量重解白板
- **发现**: 2026-08-19 · `fix/ui-jank-pty-scan`（同上）
- **位置**: `Sources/Mac/Services/CrewLocalMentionWaker.swift` 的 `directoryChanged` 扇出；同款注释「读增量靠游标，很廉价；与 listen 路同款策略」。
- **问题**: 目录事件不带文件名，所以一个 tick 要把 `watched` 里每个 crew 都扫一遍；而「扫」= 取文件锁 + 整份读 + 整份 JSON 解码，**游标只裁剪解完之后的行，读和解一分钱不省**。本机白板目录 67 个 json / 3.8 MB，全量走一遍实测 **9～11 ms**，全在主线程；helper 子进程每发一条 `post_to_crew` 就是一个 tick。
- **本次做了什么**: 给唤醒器补上 `FileChangeGate` 文件指纹门（那正是 #443 建它时写明的用途）：9～11 ms → 0.07～0.10 ms。
- **留着的尾巴**: 注释里点名的「listen 路同款策略」**没查**，很可能同病；另外 `LocalWhiteboardStore.list` 本身仍是「每次调用整份读+整份解」，没有按文件指纹缓存解码结果 —— 指纹门只是让**不必要的调用**不发生，真正需要读的那次仍然是全量。白板越长这一下越贵（本机最大的一份已经 860 KB）。

### ✅ 卡顿修复的现场复采（**已还，2026-08-19**）
- **原问题**: 四条修改的「修前/修后」数字只到函数级（`swiftc -O` 实测 + 耗时红线单测），没有对线上进程 `sample` 复采过 —— 因为复采要先装一次新版 app。
- **已完成**: 新版（0.1.13 / 20684.16770，`BuildStampCommit` = `a4f8f5d` = 当时的 main HEAD）装好后，对真进程做了四份符号化采样，两个场景都覆盖到了。结果贴在 `docs/internal/2026-08-19-ui-jank-profile.md` 的「现场复采」一节。
- **裁决**: **四条全部兑现，没有一条被推翻**。run loop 从「97.7% 在干活」变成「92～93% 空闲」；`dataReceived` 子树从 7.9%/session 变成 10 个 session 合计 1.51%（≈0.25%/忙碌 session，约 32 倍）；进程 CPU 从 69–100% 变成中位 7～8%。**场景 B（多 session 稳态）是本次第一次真正实测到，此前只是线性叠加的推断。**
- **一处措辞修正**: 原判据写「应当看不到 `squeeze` 那一族叶子」是错的 —— 那段代码本来就还该跑，正确的判据是单价，实测从主线程 94% 掉到 0.49%。
- Todo #59 据此翻 completed。

### 🟢 主线程上还剩一次目录枚举 —— `drainRenames` 每 tick 全量列目录
- **发现**: 2026-08-19 · Todo #59 现场复采
- **位置**: `Sources/Stores/LocalCrewControlStore.swift:49` `drainRenames()`，经 `CrewStore.applyPendingRenames()`（`Sources/Stores/CrewStore.swift:379`）挂在 `startRenameWatchIfNeeded` 的监视回调上。
- **问题**: 每个 tick 都 `-[NSFileManager contentsOfDirectoryAtURL:…]` 全量列一遍目录（`getattrlistbulk`），**在主线程**。与已修的第 4 条（唤醒器全量重读白板）是同一族病：主线程上的目录/文件轮询。
- **量级**: 四份采样里稳定占主线程 **0.44%–0.72%**（连外层闭包 1.34%）。比修前任何一项都小两个数量级，但**修完那四条之后，它是主线程上最大的单项**。
- **没做**: 本次只登记，没动 —— 复采那条分支的职责是验数字，不是顺手改。可仿照唤醒器补一道文件指纹门。

### 🟢 健康扫描的残余落在尾窗维护上，不在关键词匹配上
- **发现**: 2026-08-19 · Todo #59 现场复采
- **位置**: `Sources/Mac/LocalRunner/SessionHealth.swift` 的 `AnsiPlainTextTail.feed(_:)`。
- **问题**: 短语匹配改成 ASCII 字节搜索之后，**匹配本身在采样里已经看不见了**；`SessionHealthScanner.feed` 剩下的 0.47% 里约 0.4 个百分点落在 `AnsiPlainTextTail.feed` 内部的 `String.distance(from:to:)` —— 尾窗裁剪时按 grapheme 走查长度。
- **量级**: 10 个 session 合计 0.55%，可忽略。登记只是为了留个坐标：下次谁再来压这条回调，第一刀应该切在尾窗裁剪（按 UTF-8 字节裁）而不是匹配上。


### ✅ 仓库默认签名改成 ad-hoc 会静默丢登录态（**随 #63 删除，2026-08-25**）
- **发现**: 2026-08-20 · 开源准备（签名解耦）
- **位置**: `Config/Signing.xcconfig`（仓库默认值）、`Config/Local.xcconfig.example`、`project.yml` 的 `configFiles`。
- **为什么这么改**: 原来 `DEVELOPMENT_TEAM: M42BKJN82S` 硬编码在 `project.yml` 里，外部贡献者 clone 下来签不了名、编不过 —— 开源的第一道硬门槛。改成默认 ad-hoc 之后任何人都能编能跑。
- **代价转嫁到哪**: `KeychainStore`（云端 crew 的 device-grant token）的 ACL 绑当前签名身份，ad-hoc 每次重建身份就变 → 反复弹「存取钥匙串」授权框或 `-34018` 存不住 → **登录态静默丢失**。这个坑 2026-06 已经踩过一次并用「稳定的 Apple Development 身份」根治过，现在把根治手段挪到了一个 **gitignored 的文件**里。
- **谁受影响**: 只有要动云端登录/钥匙串那条路径的人。只跑本机 crew（起 claude / codex 子进程的主路径）完全不受影响 —— 那条路径不碰钥匙串。
- **失败长什么样**: 不报错。app 编得出、装得上、跑得动，只是登录态存不住。所以**症状和病因隔着十万八千里**，别再从后端/Supabase 那头查。
- **该怎么还**: 构建期没有可靠判据区分「贡献者本来就该 ad-hoc」和「本机开发者忘了装覆盖」，所以没加编译告警（那会给每个贡献者的每次构建都挂一条黄色噪音，反而训练人无视告警）。真要还，正确的地方是**运行时**：走云端登录路径时若检测到 ad-hoc 签名（`csops` / `SecCodeCopySigningInformation` 读不到 team identifier），直接在界面上说清「这个构建签名不稳定，登录态存不住」，而不是让它静默失败。
- **为什么结掉（2026-08-25 · Todo #63）**: PendingCrew 不再登录到任何地方，登录入口整块删了 —— **没有任何路径会再往钥匙串写登录态**，这条债咬不到人了。`KeychainStore` / `FamilyCredentialStore` 当时还在（跟凭据层一起等第二期），**2026-08-26 第二期已连文件一起删掉** —— 全仓再无任何 Keychain 调用。签名默认值本身（ad-hoc）不变，也不需要改。`Resources/PendingCrew.entitlements` 里那两个 keychain 组留着没删（动它可能影响本机签名，收益为零），注释里已如实写明「当前没有消费者」。
- **什么情况下要复活这条**: 哪天 PendingCrew 又要存跨启动的凭据，这条原样有效，别重新踩一遍。
- **发版不受影响**: `scripts/release/build-macos-update.sh` 在 xcodebuild 命令行上显式传 `CODE_SIGN_STYLE=Automatic DEVELOPMENT_TEAM=…`，命令行优先级最高，Developer ID 分发路径与这里的默认值无关。

### ✅ 没有 CI（**已还，2026-08-21**）
- **发现**: 2026-08-20 · 技术栈梳理（只读盘点）
- **位置**: `.github/` 下只有 `ISSUE_TEMPLATE/` 与 `pull_request_template.md`，**没有 `workflows/`**；仓库根也没有 Makefile / justfile / pre-commit。
- **问题**: `CONTRIBUTING.md`「六条硬规矩」的第 1、2 条把三件事定成硬规矩 ——「改了 `project.yml`（含新增 Swift 文件）必须 `xcodegen` 并提交 `.pbxproj`」「三端都要编一遍」「跑测试」—— 但没有任何自动化在 PR 上核这三条。其中第一条**已经踩过并且症状是「只有别人的机器编不过」**（`CONTRIBUTING.md` 第 1 条自己写着「**在别的机器上编不过**。这条真的踩过。」）：提交者本机 Xcode 会自动发现新文件，所以他永远看不到红。
- **为什么现在要记**: 之前仓库只有作者一个人、一台机器，靠纪律够用。开源之后进来的每个 PR 都是「另一台机器」，而这正是这条规矩失效时唯一会暴露的场景。
- **代价转嫁到哪**: 维护者的人工 review。pbxproj 漂移与 iOS 端静默打红这两类问题都不会在 PR 页面上显形，只能靠维护者自己 checkout 下来跑三条命令。
- **该怎么还**: 一个 macOS runner 上的 workflow，三步即可覆盖：`xcodegen && git diff --exit-code PendingCrew.xcodeproj/project.pbxproj`（抓漏 regen）、macOS build + test、iOS Simulator build。测试跑满约 3 分钟（2026-08-20 本机实测 184s / 1443 tests）。**注意**：`CrewChatOpenCostTests` 在 CI 上会 skip（fixture 不入 git），这是预期的，别为了让它绿而把 fixture 提交进去。
- **已完成（`.github/workflows/ci.yml`）**: 两个 job —— ①「pbxproj 与 `project.yml` 同步」重跑 xcodegen 后比 diff；② 三端编译 + 单测。落地过程中还顺带查出并根治了一个真问题：**仓库根目录的 xcconfig 会让 `xcodegen` 输出不确定**（`project.pbxproj` 里那条文件引用的 uuid 每次 regen 都变），那道 diff 检查因此永远红 —— 修法是把 xcconfig 挪进 `Config/`（`4dc855a`）。
- **一处措辞更正**: 上面「测试跑满约 3 分钟」是**本机热构建的测试执行时间**，不是 CI 的墙钟。CI 是冷机，要连 SPM 解析和两轮全量编译一起算。别拿本机数字当 CI 预算。

### 🟢 `docs/architecture.md` 还差这几处 —— 逐处名单，别重新考古
- **发现**: 2026-08-25 · Todo #63 第一期；2026-08-26 第二期扩大一次并**改掉了其中的事实性错误**
- **已经改掉的（2026-08-26，一笔独立提交）**: 34–37 行的正文口径、目录表里两条已不存在的
  目录（`Sources/Auth/` / `Sources/Remote/`）、`Sources/Services/` 的职责描述、
  「11 个非 macOS 专有文件」里那条已删的 `CrewInteractionCard.swift`、vendored 一节里的
  `Sources/Auth/` 与 `AttachmentDownload`、测试地图里的 `CrewHostedConfigTests` 行、
  8.3 的 skip 表（11 → 10）、长期服务列表里的 `CrewRelayAgent`、P0 那段
  `CrewMailboxWaker` / `SessionPermissionRelay` 的时态、速查表里的 `CrewHostedConfig` 行。
- **故意没动、要留着的**: **第 2 节的依赖表与第 3 节整节**（约 164–290 行）。那不是「过时的
  描述」，是**删除决策的证据** —— 删了或按新数字重写，将来的人只看得到「这里曾经有过依赖」，
  看不到「为什么删」。两节节首各加了一条带日期的横幅说明数字是 2026-08-25 之前的实况。
  第 269 行那句「判据是 `CrewHostedConfig` 的四个占位常量」落在这个范围里，同理不动 ——
  单独挖掉它会把证据链弄断。
- **还差什么（逐条，下一个人照着做即可）**:
  1. **重测规模数字**：第 39–41 行的「249 个 Swift 文件 / 48,038 行 / 1,443 个 `func test`」
     是 2026-08-20 实测，#63 两期删掉约 20 个文件之后没重跑。已在原地标了日期，但没改数。
  2. **重测两张目录表**：第 4 节的「文件数 / 含 `#if os(macOS)` 的文件数」与 5.1 的行数表，
     同样是 2026-08-20 的数，只删了已不存在的目录行。同样已标日期。
  3. **写一节「现在的架构长什么样」**：第 2、3 节讲的是删除前的成本结构，删除后**没有任何
     一节讲现在的依赖构成**（5 个直接依赖、还剩几个 pin、体积多少）。这是唯一需要**新写**
     的一块，所以第二期没动 —— 边界是「只改事实、不动作者行文」。
  4. **README「状态」一节**：第 346 行原来指着它说 `Sources/Remote/` 未接通，那条已删；
     README 本身在作者手里有未提交改动，两期都没碰，落地后要看一眼口径还对不对。

### 🟢 `Sources/Mac/` 名不副实，而且没有任何编译期的「层」
- **发现**: 2026-08-20 · 技术栈梳理（只读盘点）
- **位置**: `project.yml` 的 `PendingCrew` target 只有一条 `- path: Sources`；`Sources/Mac/` 下 110 个 swift 文件里 **11 个不含 `#if os(macOS)`**。
- **问题（两条，互相放大）**:
  1. **一个 target = 一个 module**，`Sources/` 下的目录只是目录，没有 `import` 边界。所谓「分层」全靠约定，编译器一条都不管。
  2. **`Sources/Mac/` 里混着跨平台文件**，于是别的目录必须反向引用它才能拿到那些类型：
     - `Sources/Mcp/McpServer.swift:640` 用 `AgentQuotaFile`、`:1043-1083` 用 `AgentModelCatalog` 一族（都在 `Sources/Mac/LocalRunner/`）
     - `Sources/Support/QuotaRingLayout.swift:62,84` 用 `AgentQuotaSnapshot` / `AgentQuotaWindow`（同上）
     - `Sources/Chat/Adapter/CrewComposerMentions.swift:358` 用 `CrewSenderResolver`（在 `Sources/Mac/Views/Chat/`）
     - `Sources/Views/IPadShell.swift:47` 直接构造 `CrewChatView`（`Sources/Mac/Views/CrewChatView.swift`，1437 行，**两端都编**，它就是 iPad/iPhone 的群聊页）
- **为什么不是 🟡**: 那几个文件的头注释都明写了「纯 Foundation、不带平台门 —— McpServer（跨平台编译）要用」，是**有意为之、只是放错了目录**，不是把 macOS 代码偷渡进跨平台路径。所以它腐蚀的是可读性，不是正确性。
- **失败长什么样**: 新人（含三个月后的作者）按目录名判断「这是 macOS 专有的、我随手 import 个 AppKit」→ iOS 端静默打红，而且只在别人跑 iOS 构建时才发现。
- **该怎么还**: 把那 11 个文件挪到 `Sources/Support/` 或新建的 `Sources/Agent/`；`CrewChatView` 归到 `Sources/Chat/` 或 `Sources/Views/`。纯搬家、无行为变化，但会动一批 import-free 的引用点和 `project.yml` 的测试文件清单，属于**大改一批文件**的动作，不要顺手夹在功能分支里。真正的分层保证（拆 target / SPM local package）代价大得多，不在这条的范围内。

### 🟢 `whiteboards/` 目录只增不减 —— per-session 文件从不回收
- **发现**: 2026-08-20 · 技术栈梳理（只读盘点，复核了 2026-08-18 那份调查）
- **位置**: `~/Library/Application Support/PendingCrew/whiteboards/`；产生方 `Sources/Mcp/WhiteboardCursor.swift`（`.cursor` / `.cursor.lock`）与 `Sources/Mcp/SessionTurnTrace.swift`（`.turn`）。
- **问题**: 每起一个 session 就多三个小文件，**session 退出后没有任何清理**。已有一份逐项盘点与清理方案：`docs/internal/2026-08-18-whiteboards-directory-cleanup-plan.md`（当时 1051 个文件，其中约 341 个属于早已不存在的成员）—— 但那份文档写明「本轮只调查、只出方案，一个文件都没删、没移、没改」，所以**账上一直没有一条活的登记**。
- **证据（本次实测）**: 同一目录今天 **1346 个文件 / 36 个 crew**。两天涨了约 295 个。
- **为什么值钱的不是磁盘**: 主线程仍在列这个目录 —— 见上面那条 🟢「`drainRenames` 每 tick 全量列目录」（`getattrlistbulk` 的开销直接乘以文件数）。文件数是那条的乘数。
- **没做**: 本次是只读梳理，一个文件都没动。真要做的话方案已经在上面那份 internal 文档里写好了，包括「`.corrupt-*` 是 8-12 事故的现场证据、该先移进归档目录由人拍板再删」这条纪律。

### 🟢 驾驶舱有一半的数据契约只存在于另一个仓库
- **发现**: 2026-08-20 · 技术栈梳理（只读盘点）
- **位置**: `Sources/Models/CockpitModel.swift:362-383` 的 `CockpitLoader.load`；空态文案在 `Sources/Mac/Views/CockpitView.swift:112`。
- **问题**: 驾驶舱从 **crew 的工作目录**读四样东西：`docs/roadmap.md`、`docs/handbook/`、`docs/state/`、`docs/tasks/`。其中 `docs/roadmap.md` 缺失时有一份**自带格式模板的空态引导**（`Sources/Mac/Views/CockpitRoadmapView.swift:369-393`），照着建就能用；但 `docs/handbook/` 与 `docs/state/` 的格式**这个仓库里没有任何地方写过**，而 `Sources/Mac/Views/CockpitView.swift:112` 的空态直接告诉用户「让某个 crew 的工作目录指向带这些账的仓库（比如大绿豆自己）」—— 那是另一个**未开源**的仓库，外部贡献者拿不到，也无从照着造一份。
- **牵连**: `README.md:69` 把「驾驶舱」列在「真跑过、天天在用的」里，没有任何限定语。对一个把工作目录指向自己 clone 的人来说，驾驶舱的任务段能用（人类 Todo + `~/.claude/tasks` 都在 app 数据目录），路线段照引导建一份 `docs/roadmap.md` 也能用，**但期望/现状那两栏永远是空的，而他不知道为什么**。
- **该怎么还**（三选一，都不大）: ① 给 `docs/handbook/` 与 `docs/state/` 也补上同款自带模板的空态引导；② 在 README 的能力清单里给「驾驶舱」加半句限定；③ 把这两本账的格式写进 `docs/`。**别改代码去删功能** —— 它对作者本人是天天在用的。

### 🟢 「加载更早」那一帧：查清了，也修了 —— 但「未走的路」是走不通的那条

- **发现**: 2026-08-23 · 人类 Todo #60；**2026-08-26 返工并了结**（人类验收没过）
- **位置**: `Sources/Mac/Views/CrewChatView.swift` 的 `expandEarlier` / `topAnchorBox` /
  `ChatScrollAnchor`；探针在 `Tests/PendingCrewTests/CrewChatWindowTests.swift` 的
  `CrewChatExpandAnchorProbeTests`。

#### 当初那条警告，兑现了

原文一字不改留在这儿：

> **第 4 条要是真出了问题，正确的方向是走这条未走的路（先消灭翻面 + anchor），不是给
> `scrollTo` 加补丁。别被「anchor 试过了不行」误导 —— 它没被试过。**

**这句在 2026-08-26 兑现了：它被试过了。** 留着它不是为了记账 —— 一条警告完成它使命的
方式就是变成一条结论，删掉它等于告诉后来者「写了也会被覆盖掉」。

两处要接着往下写，因为**兑现的方式和当初预想的不一样**：

1. **它的触发条件从来没被满足。** 那句「第 4 条」指的是清单里「点完静置两秒，看懒行真实
   高度回填之后锚点漂没漂」。人类照着念了，回答是「**位置倒是一样**」—— 第 4 条过了，
   锚点没漂。真正出问题的是清单**没问**的那件事。
2. **它推荐的那条路走不通**，见下。所以它对方向的判断（「别给 `scrollTo` 加补丁」）是对的，
   对手段的判断是错的。

#### 真凶：那记补偿结构上必然晚一帧

原探针量首尾（不补偿跳 680pt、补偿后 14pt / 1pt），**从来没量过中间经过哪里**。补上路径
探针（判据是 CoreAnimation 的提交边界，一次提交 ≈ 一帧），起点 694：

| 点击那一下 | 第一帧 | 第二帧 |
| --- | --- | --- |
| 什么都不做（对照） | 1374 | 1374 |
| 上一版 `scrollTo` + 主线程 hop | **1374** | 680 |
| 现在（`.scrollPosition(id:anchor:.top)`） | **698** | 698 |

**两趟的第一帧是同一个数** —— 那记 `scrollTo` 一帧都没提前。而且**没有动画**
（offset 56→740 一步到位），所以人类那句「滑」不是动画，是 680pt 出现一帧又弹回去。
这不是补偿量不够，是结构性的：`renderLimit` 写下去那一刻新的一页还没进视图树，
同一拍 `scrollTo` 抓不到目标 ⇒ hop 必需 ⇒ 两者必然落在两次提交里。

#### 「未走的路」：走过了，走不通

那条路写的是「内容在上面长、锚底部 = 视口一像素不动，原生机制」。**实测不成立。**
`.bottom` 的语义是「把视口钉在内容底部」，不是「保持与底部的相对距离」—— 它只在视口
**已经贴底**时做事，而「加载更早」的现场按定义就是人已经滑上去了
（`anchorOnExpand` 在跟随时返回 nil），正是它不响的那个现场。人滑上去之后 `.bottom` 与
`.top` 读数**逐字相同**（都是 1382）。就算它响了，它会把人一把拽到底部，比原来还糟。

这不是「尺子没量到锚」：同一把尺子，视口**贴底**时两个锚值差 **707pt**。
用例：`test_未走的路_人滑上去之后bottom锚与top锚读数相同` + `test_锚标定_贴底时必须分得出bottom和top`。

#### 落地的那条：`.scrollPosition(id:anchor: .top)`

点击那一拍先把「顶上那条」的 id 写进绑定，再改上限，**两件事落在同一次 body 更新里**。
没有 hop、没有程序化 `scrollTo`，也就没有「被拽回来」这个动作 —— **不是把中间帧修小，
是让它没有理由存在。**

两条容易被下一个人误删的细节，都量过：

- **那一笔写入是承重的，不是防御性的。** 只挂绑定不写 id → 偏 **680pt**（等于没修）；
  多静置一拍 → 仍然 680pt；写了 id → **24pt**。用例 `test_诊断_按住位置靠的是哪一步`。
- **绑定落进一个不被观察的引用盒子，不是 `@State`。** 回写 30 次：落进被观察的存储 →
  body 求值 **30 次**；落进盒子 → **0 次**。它照样生效，因为紧接着 `renderLimit` 那一改会
  引发更新，SwiftUI 在那次更新里读 getter，正好读到刚写的值。

#### 还欠着的三件

1. **容器身份翻面还留着。** `usesEagerInitialLayout` 的 `limit <= pageSize` 判据仍会让
   12→24 那一下换容器、整棵树重建。**它不再是承重项**：翻面在场第一帧偏 −24pt，消灭之后
   偏 +4pt —— 从 680pt 降到 21pt 的差。消灭它要动首屏 eager 测量那条路（Todo #56 的地盘），
   是独立一笔。**代价**：第一次点击比后续每一次多 21pt 的位移。
2. **`.top` 那一臂是不是空转，未决。** 人已经滑上去、内容在下面长时，挂 `.top` 与
   **不挂任何锚**读数相同（都是 +673，差 0）—— 也就是**读数分不出它做没做事**，倾向空转。
   ⚠️ **不许据此写成「它是死代码」**：没做「删掉它再量」那一步，一次读数不成立不构成证伪
   （这条 tech-debt 自己刚在 anchor 上栽过同一个跟头）。用例
   `test_记录_松开跟随时top那一臂是否在做事` 只打印不断言。**别为它改代码。**
3. **人眼那一关还没过。** 探针证明的是「离屏下第一次提交画的已经是终点位置」；
   **「真窗口里人眼看不看得见」的唯一证据，始终是人类那句话。** 两者互相支撑，但不是同一个
   证据 —— 合并了就没有外部校准点了，而当初那套探针出问题的最后一块，正是它自己说自己是对的。

#### 尾巴：给人念的清单，别再问探针已经能答的东西

当初为这件事写过一份「人类装完更新后照着念」的四条清单，人类真念了、四条都答了，**但他报
回来的那个问题四条里一条都没问到** —— 四条问的全是「位置对不对」。**清单继承了探针的盲区。**
这条已经作为实例三写进 `CONTRIBUTING.md` 第 5 条，不在这儿重复。

所以这次不再列坐标清单，只留一句给人类：**点几次「加载更早」，看它是不是「本来就在那儿」，
而不是「移过去的」。** 位置对不对不用他核，探针能答。

#### 尾巴二：一个**从未被定位**的红，别当它已经被治好了

某次全量跑里见过 **1 个 failure**，把探针改成「等几何量连续 25 拍不变」之后就没再复现过
—— **但它具体是哪条用例、为什么红，从头到尾没有定位到**。

所以：**这一族将来再冒红，不许默认已经被 `788fca9` 治好。** 没抓住的红不算被治好，
只算没再出现。

### 🟡 机长作战板对人类只读 —— 是**刻意推迟**，不是没想到
- **发现**: 2026-08-25 · 人类 Todo #66 A 段（`CockpitPlanStore` / `plan_*` 三个 MCP 工具）
- **位置**: `Sources/Stores/CockpitPlanStore.swift`（唯一写入口在 `Sources/Mcp/McpServer.swift` 的 `guard isCaptain` 后面）。
- **现状**: 第六本账「机长作战板」**只有机长写得动**：人类在驾驶舱里能看，不能改、不能追问、不能翻状态。这是按规格做的 —— 这本账的定位就是「机长自己整理的」，与人类 Todo 那两本（人类写 / agent 写）方向不同。
- **为什么记在这**: 人看到一条写错的计划必然想动手，而**「不能改」和「不知道怎么改」是两回事**。UI 上必须明说怎么让它改（一句「让机长改」或者把这条带进群聊输入框的入口），否则这块板在人眼里就是死的 —— 这是 B 段（第三个药丸 + 面板）必须带上的一条，不是可选装饰。
- **推迟的是什么**: 双向编辑（人类直接改/追问机长的计划）。第一版不做，理由是别在方向都没跑顺之前就把它做成双向；不是没想到。
- **要动的时候怎么动**: 人类那一侧照 `LocalTodoStore` 的 `followUp` 语义走（追加式、不覆盖、任何状态都能追问），别新造第二套编排。

### 🟡 `CrewChatOpenCostTests` 里的**计时断言在飘** —— 一族，不是一条
- **发现**: 2026-08-26 · 侧栏「手动藏起来一个 crew」落地时取全量基线，撞上前提对不上
- **位置**: `Tests/PendingCrewTests/CrewChatOpenCostTests.swift` —— **已撞到三句，两种形状**：
  - `:493` `XCTAssertLessThan(cost, budgetMs)` — 绝对毫秒预算
  - `:342` `XCTAssertLessThan(costWindowNoSel, budgetMs)` — 绝对毫秒预算（同型）
  - `:346` `XCTAssertLessThanOrEqual(costWindowNoSel, costWindow)` — **相对比较，跟 100.0 无关**
- **机制（2026-08-26 改写过一次，别退回旧版）**: 不是「绝对毫秒预算对负载敏感」——**那个表述盖不住 `:346`**。真正共有的是「**在功能全量里做计时测量，而第一次触碰是冷的**」：
  - 绝对预算（`:493` / `:342`）：冷跑把数值整体抬高 → 越过常数；
  - 相对比较（`:346`）：冷启动**抬得不均匀** —— 同一个 test 里几个测量点谁先跑谁吃冷启动，于是 D 可能比 B 贵，**即使代码上 D 更省**。
  **写成「绝对预算」那版的后果是可预见的：下一个人按那条还法把预算调宽或门控掉，`:346` 照样飘。**
- **实测读数，每条写具体那一趟（commit + 毫秒 + 单跑/全量），不写「与基线一致」**:
  - `:493` — **2026-08-25** `main` @ `e17b268` / `ab0942c` 两趟全量：**红** `112.464084` / `111.977917`；**2026-08-26** `main` @ `3dade35` 两趟全量：**绿**；同日 `4e30434` 全量 **绿 0.103s**、`7c39f39` 全量 **绿 0.256s**。
  - `:342` — **2026-08-26** `b8bd679`（0.1.16 发版闸门，钉死 worktree）全量：**红 2.453s**；**同一二进制单跑：绿 0.422s**。同日 `7c39f39` 全量：**红 109.73**（邻居 `:493` 同趟 0.256s，2.5×）；单跑三次 **102.4 红 / 42.3 绿 / 39.6 绿**。
  - `:346` — **2026-08-26** `4e30434` 单跑三次：**86.2 红**（`86.165874 > 66.62446`）/ 56.1 绿 / 29.2 绿。
  - **冷 vs 热（实测，不是推的）**：六次单跑里**两次红都是各自 checkout+重编之后的第一趟**；同格「修之前」冷跑 598.1 / 309.6 ms，热跑远低于此。**全量的第一次触碰天然是冷的。**
- **判据（照这个走，别重新设计实验）**:
  1. **单跑绿 + 全量下红** ⇒ 属本族，不是回归；
  2. **配对载荷计**（成本为零，数已经在日志里）：每个全量样本**同时记目标 + 邻居 `:493` 两个毫秒数**。目标红而邻居也慢 ⇒ 那趟机器忙；**目标红而邻居正常（~0.1s）⇒ 往回归想**；
  3. **跨 commit 出现过**：`:342` 在 `b8bd679` 就红过，而它之后的 `c24eef8`/`f17ae13`/`7c39f39` 三笔与它无关 —— **一条在更早 commit 上的红，比任何 A/B 对照都强，因为它不需要对照组。**
- **问题**: 于是它**既不是稳定红也不是稳定绿**。危害不在这条用例本身，在它会**污染别人的对账**：2026-08-25 之后「这条在 main 上本来就红」被当成既有结论沿用，下一个人拿它当前提时前提已经不成立了；反过来，谁哪天撞见它红，也很容易以为是自己改出来的。
- **别把它跟 fixture 那两条混成一件事**（本仓库现有两处讲的都是 fixture，与本条无关）：`CONTRIBUTING.md`「有几个测试需要现取 fixture」那节的「`CrewChatOpenCostTests` 用的是真实群聊数据（不入版本历史，见 `.gitignore`）」、本文件「没有 CI」那条里的「`CrewChatOpenCostTests` 在 CI 上会 skip（fixture 不入 git），别为了让它绿而把 fixture 提交进去」。**那两条说的是「在 CI 上跑不了」，这一条说的是「在本机跑得了、但结果在飘」。**
- **另有一条同类（别合并成一条）**: 下面那条 `CrewLocalImageCacheTests` 也是「在飘」，但**机制不同、还法也不同** —— 那条是 `NSCache` 的可回收语义，这条是冷启动下的计时测量。读到任一条的人应该知道还有一条。
- **还有一条同类，而且方向相反（更别合并）**: `LayoutLoopRegressionTests:245` 的 `testSwiftUIRepeatForeverInLazyListSelfExcites` 也在飘，但**这一族量耗时、忙让数变大→越过上界**，那条**量固定窗口内的次数（是速率）、忙让数变小→跌破下界**。**一个机制解释不了两个相反的方向**，别把「冷启动首触」这套搬过去。见本文件下方那条。
- **为什么这条按「一族」记而不是一条一笔**: 三句共用一个机制、一条还法。**债本里一族洞开三条账，等于把一次修复拆成三次判断** —— 再撞到第四句，追加进本条的实测清单，**不要新开。**
- **该怎么还**: **先加一趟丢弃的预热**（测量前先跑一次、不断言）—— **只有这一条同时修好两种形状**：它既压掉绝对预算那边的冷启动溢出，也消掉相对比较那边「谁先跑谁吃冷启动」的不均匀。其余几种（相对基准 / 多次取中位数 / 挪进专门的性能 job / `XCTSkipUnless` 门控）**只修得了绝对预算那种，`:346` 那类照样飘** —— 别只做这几种就当还完了。总之别让计时测量混在功能全量里给出会飘的红绿。**在改成不飘之前，任何人拿「这条在 main 上是红/绿的」当前提，都要当场重跑一趟核实。**

**观测记录 —— 每次「真跑过」就追加一行，绿也要记。**

> **为什么绿也要记**：只记红，这张表就永远只能证明它飘 —— 那正是「已知飘红」这个标签
> **自我实现**的机制。贴上标签的那一刻，这条测试在两个方向上都不再携带信息：红了「它本来就飘」，
> 绿了「一趟绿不算数」。**它就此退出了检测器的行列，只是没人宣布过。**
>
> **必须填条件那两栏**：这是负载敏感断言，**没有边界条件的读数不构成证据**。
>
> **退出条件（摘掉「飘红」标签的门槛）**：连续 **3** 次在**有负载**环境
> （同时 ≥4 个 `xcodebuild` 在跑）下通过 → 重新评估这个标签。
> 取 3 的依据：账上那次红是在 19 个并发下发生的，1 次绿分不清「运气」和「趋势」，
> 3 次是这个跑动频率下最小的可判样本。**这个数可以改，但必须有一个数**
> ——否则这个标签永远摘不掉。

| 日期 | 环境 | 结果 | 耗时 | 同时在跑的 xcodebuild |
|---|---|---|---|---|
| 2026-09-07 早 | 共享树 `e4033dc`，机器较空 | **passed** | **31.8 ms** / 预算 100 ms | **未量**（当时没数——第一行就缺这一栏，正好说明这张表为什么需要它） |
| 2026-09-07 01:5x | 共享树 `0dea14e`，多条线并行合并、10 个 crew 刚被同时叫醒 | **FAILED** | **161 ms** / 预算 100 ms | 高（当晚最忙的时段） |

> **⚠️ 这张表的「指标」栏填的是被测量本身（那条 `B + D` 的毫秒数），不是 XCTest 报的用例耗时。**
> 第一行原来误填了用例耗时 `0.324s` —— 两者差一个数量级，并排会让人以为它变慢了 500 倍。
> 真指标在测试自己打印的那个方框里（`║ B + D ... 31.8 ms` / `║ 预算 100.0 ms`），**通过时也打**，
> 别只在失败信息里找。
>
> **两行放在一起才有意义**：同一条断言、同一台机器，**空载 31.8 ms / 高负载 161 ms —— 5 倍**。
> 到这里，「对机器负载敏感」不再是猜测，是**两个读数之间的差**。
> **但先别动那个 100 ms 预算**：两个样本不够判断这个数合不合理，先按上面的退出条件攒够再谈
> 「抬预算」还是「真去优化」。

### 🟡 `CrewLocalImageCacheTests` 两条**在飘** —— 断言把 `NSCache` 当成了「存了就一定在」
- **发现**: 2026-08-26 · 侧栏「手动藏起来一个 crew」落地时，在共享目录取合前基线撞上

**实测到的**（同一台机器、同一天，两趟全量之间隔约 20 分钟）:

- `main @ 6b3db2f` 一趟：**两条红**，`Executed 1643 tests, with 3 tests skipped and 2 failures`。
  ```
  Tests/PendingCrewTests/CrewLocalImageCacheTests.swift:33: error: -[PendingCrewTests.CrewLocalImageCacheTests testStoreThenPeekHits] : XCTAssertTrue failed - 同一 key 必须命中同一张，不该重解
  Tests/PendingCrewTests/CrewLocalImageCacheTests.swift:59: error: -[PendingCrewTests.CrewLocalImageCacheTests testDifferentMaxPixelIsDifferentEntry] : XCTAssertNotNil failed
  ```
  断言原文（`CrewLocalImageCacheTests.swift`，逐字）：
  ```swift
  XCTAssertTrue(cache.peek(key) === image, "同一 key 必须命中同一张，不该重解")   // :33
  XCTAssertNotNil(cache.peek(thumbKey))                                          // :59
  ```
- `main @ aa05a2a` 一趟：**两条绿**，`Executed 1670 tests, with 3 tests skipped and 0 failures`。
- 两趟之间落地的是侧栏可见性那六笔，**没有一笔碰 `Sources/Mac/Support/CrewLocalImageCache.swift` 或它的测试**。

**读代码读到的**（打开文件看过的那两行，不是推的）:
`Sources/Mac/Support/CrewLocalImageCache.swift:52-53` —— 底座是 `NSCache<NSString, NSImage>`，
文件自己的注释写着「`NSCache` 自带线程安全 + **内存压力下自动清空**，按像素字节数计成本」。
也就是说 `store` 之后 `peek` 返回 nil 是 `NSCache` 的**合法行为**，不是 bug。

**排掉「是不是 cost limit 设小了」这条**（同样是打开文件看到的，省下一个人重新去猜）：
`CrewLocalImageCache.init(costLimitBytes: Int = 64 * 1024 * 1024)`，而每个用例都是
`let cache = CrewLocalImageCache()` **新实例**、存的是 200×200 / 400×400 且已降采样到
≤100px 的小图 —— **不可能是自己撑爆 cost limit**，只可能是**系统级内存压力**触发
`NSCache` 全局清空。这跟「跑全量的那台机器同时在跑别的活」对得上，也解释了为什么它
偏偏在取基线那趟撞上。

另一条同层的：**全文件只有两处断言依赖「存了就一定在」，红的正是那两处**
（`:33` 的 `peek(key) === image`、`:59` 的 `XCTAssertNotNil(peek(thumbKey))`）。
同文件里其余几处 `peek` 断言全是 `XCTAssertNil`（"还没存过" / "覆盖后不该拿到旧解码
结果" / "看大图不该拿到缩略图"），对回收免疫，所以一次都没飘过 —— 这条对应关系本身
就是这个诊断最硬的一块。

**推出来的（标明是推的，没验证过）**:
- 「那两趟的红是全量满载下 `NSCache` 真被回收了」—— 机制说得通、也与 `peek` 返回 nil 的症状一致，
  **但我没有在回收发生的那一刻抓到证据**（没加计数器、没复现）。只跑到「实现允许这件事发生」为止。
- 「本次侧栏改动与图片解码缓存无交集」—— 依据是两条路没有共同调用点（侧栏可见性 vs 图片解码），
  **是从文件与职责推的，没有跟到调用链级别去证**。

- **同 commit 的一红一绿（2026-08-26 · `#14 reply_to 接自动@`）—— 「在飘」到此不必再排干扰项**:
  同一个 worktree、同一台机器、**同一个 commit `622fc8d`**，两趟全量隔约 20 分钟：
  **第一趟这两条红，第二趟绿**（`Executed 1690 tests, with 11 tests skipped and 0 failures`；
  两趟跑前跑后 `HEAD` + `git diff | shasum` 都逐字未变）。第一趟红时按本条判据单跑
  `-only-testing:CrewLocalImageCacheTests` **6 条全绿 0.08s**。
  **这对样本的价值在于它不需要对照组**：本条上面记的那对红绿在**两个不同 commit** 上
  （`6b3db2f` 红 / `aa05a2a` 绿），所以必须先花一段排「是不是那几笔改出来的」；
  **同 commit 的一红一绿没有这个退路可排。**
  ⚠️ **边界，跟证据一起读**：这对样本**只坐实「在飘」，没有指向机制** —— 它跟上面
  「`NSCache` 真被回收了」那条推测**方向不矛盾**，但两趟之间**没有任何内存压力的观测**，
  所以**不构成对该机制的确认**。「方向不矛盾」不是证据，是没有反证。
  ⚠️ **别把本条上方那条 `CrewChatOpenCostTests` 的「冷启动首触」搬到这条上** —— 见那条末尾
  「另有一条同类（别合并成一条）」：**那条是冷启动下的计时测量，这条是 `NSCache` 的可回收语义。**
  照冷启动那条的还法（加一趟丢弃的预热）来修这条，修不动。

- **为什么记**: 危害不在这两条用例本身，在**它们会污染别人的对账**。任何人拿全量差分判断
  「我这一改打红了什么」时，一条会自己红自己绿的用例就是一个假信号 —— 而它红的时候看起来
  非常像真 bug（"同一 key 必须命中同一张"）。
- **该怎么还 —— 两条用例情况不同，分开办**:

  **`testStoreThenPeekHits`（:33）不许改成条件断言。** 整条用例的存在理由就是「存了要
  命中」，把它写成 `if let hit = cache.peek(key) { XCTAssertTrue(hit === image) }`，在
  `peek` 恒返回 nil 时照样通过 —— **一个名字承诺「会命中」、断言却不再检查命中的测试**，
  正是本仓库反复在清的那种安静的死：编译器不报、测试不红、看代码也看不出来。两条正路：
  - 给缓存留一道缝：把存取抽到一个可注入的协议后面，测这条契约时用一个**不会被系统回收**
    的实现（一个普通字典就够）。`NSCache` 那半留给真跑的路径。
  - 或者**改名 + 注释写明**「`NSCache` 不承诺命中，本例只钉『命中时必须是同一张』」。
  **二选一，不许只改断言不改名** —— 名字和断言必须对得上。

  **`testDifferentMaxPixelIsDifferentEntry`（:59）可以放宽，但只放宽一半。** 它真正要防的
  是最后那句 `XCTAssertNil(cache.peek(fullKey), "看大图不该拿到 100px 的缩略图")` ——
  **那句对回收天然免疫**（回收只会让它更容易过）。飘的只有前面那句
  `XCTAssertNotNil(cache.peek(thumbKey))`，它在这条用例里只是个前置铺垫，不是被测契约。
  放宽它，并**在注释里写清楚为什么只有这半可以放宽**，别让下一个人照着把上面那条也放宽了。

  **一条别走的弯路**：测试里那个 `let image = ...` 的强引用**挡不住** `NSCache` 在内存
  压力下清空 —— 别以为多持一个引用就修好了。

- **另有两条同类（别合并成一条）**: 上面那条 `CrewChatOpenCostTests` 也在飘，机制是
  **在功能全量里做计时测量、而第一次触碰是冷的**（别写成「绝对毫秒预算对负载敏感」——
  那个表述盖不住它的 `:346`，见那条自己的说明）；下方 `LayoutLoopRegressionTests:245`
  那条则是**速率判据跟负载耦合、方向相反**。三条的机制与还法各不相同。

### 🟡 `LayoutLoopRegressionTests` 那条自激判据**在飘** —— 量的是速率，却跟一个绝对次数比

- **发现**: 2026-08-26 · #66 B 段取全量基线时撞上
- **位置**: `Tests/PendingCrewTests/LayoutLoopRegressionTests.swift:245`
  `testSwiftUIRepeatForeverInLazyListSelfExcites` 的第二句：
  `XCTAssertGreaterThan(n, 1000, …)`（第一句 `XCTAssertLessThan(idle, 10)` 不在此列，见下）
- **实测读数（每条写清那一趟是单跑还是全量）**:
  - **929** —— 全量、机器满载：**红**（离阈值 1000 只差 8%）
  - **10105** —— 单跑：绿
  - **28917** —— 全量：绿
  **两趟全量差 31 倍**，这是本条区别于隔壁那族的硬证据。
- **机制**: 这条量的是**固定时间窗（安静窗口）内 `layout()` 被调用的次数** —— 那是**速率**，
  不是耗时。**机器越忙，同样一段墙钟时间里跑得越少，数就越小**，于是撞下界。
  自激本身有没有发生跟机器忙不忙无关，但**这把尺子的读数跟负载死死绑着**。
- **⚠️ 别把上方 `CrewChatOpenCostTests` 那族的「冷启动首触」搬到这条上 —— 方向是相反的**:
  - 那族量**耗时毫秒**，冷/忙让数**变大** → **越过上界**（`XCTAssertLessThan`）；
  - 这条量**窗口内次数**，忙让数**变小** → **跌破下界**（`XCTAssertGreaterThan`）。
  **一个机制解释不了两个相反的方向。** 而且冷启动也解释不了「同为全量的两趟差 31 倍」——
  两趟的首触都是冷的，性质相同；**机器忙不忙才解释得了。**
- **为什么另外两句不在此列**（别顺手一起改）:
  - 同一 test 的第一句 `idle < 10`：idle 实测恒为 0，**负载让它更小**，只会更安全；
  - 隔壁 `testBreathingSymbolDoesNotSelfExcite` 的 `n < 10`：同理，**负载让它更容易绿**。
  **只有「要求次数足够多」的那一句会被负载压红。**
- **该怎么还**: **跟同条件下的 idle 基线比，别跟绝对数比。** 这条 test 里**已经先量了 idle**
  （`quietTodoLayouts(breathingRow: -1)`，实测恒为 0），两个测量点同机同时同负载 ——
  把判据改成「有呼吸那趟比 idle 高出一个数量级以上」，负载对分子分母同向作用，就抵掉了。
  **别用「把阈值从 1000 调到 500」了事** —— 那只是把同一把跟负载耦合的尺子往下挪，
  下一台更忙的机器照样红，而且越挪越接近「自激真的没发生」也能过。
- **在改之前**: 谁撞见这条红，**先看同一趟日志里 `[quietTodoLayouts]` 打的 idle 数** ——
  idle 是 0 而这条只是没到 1000 ⇒ 属本条，不是回归；**idle 自己就不是 0 ⇒ 骨架真出事了，去查骨架。**

### ✅ `WorkdirMigrationPlan` 搬 transcript 那半已失去存在理由（**已还，2026-08-26 · 机长作战板 #12**）

- **在哪**: `Sources/Mac/LocalRunner/WorkdirMigrationPlan.swift` 与 `WorkdirMigrationExecutor`
  里所有与 claude 会话日志搬运相关的分支：`Action.moveClaudeTranscript` /
  `moveClaudeTranscriptSidecar`、`Skip.transcriptSourceMissing` / `transcriptTargetExists` /
  `sessionStillLive` / `codexSessionNeedsNoMove` / `unknownAgentKind`、
  `Plan.claudeTranscriptMoveCount` / `affectedMembers` / `pendingSweepMembers` / `isSweep`，
  以及**整个清扫模式**（`isSweep` + `sourceDirectory` 走 `previousWorkingDirectory` 那条分支）。
- **为什么失去理由**: **不是因为我们后来记了工作目录**，而是因为 **claude 压根不按目录找会话**。
  2026-08-26 实测（claude 2.1.246，Todo #68）：把 jsonl 挪到一个跟任何真实路径都对不上的
  目录，再换第三个目录 `--resume <同一个 id>` **照样接上**；挪到 `~/.claude/projects` 树外
  才报 `No conversation found with session ID: <id>`。官方 `--help` 划的是同一条界：
  `--continue` 写明 *in the current directory*，`--resume` 一个字都没提目录。
  **搬它零功能收益。** 完整查实见 `docs/internal/2026-08-26-session-resume-workdir-evaluation.md`。
- **必须留，别一起删**（⚠️ **2026-09-08 被推翻了一半，见下面那条**）:
  `copyClaudeProjectSettings`（`~/.claude.json` 的
  `projects["<绝对路径>"]` 信任条目）、`copyCodexTrust`（`~/.codex/config.toml` 的
  `trust_level`）、`copyClaudeMemoryFile`、`setCrewWorkingDirectory`。**这四样跟记不记
  工作目录完全无关** —— 少了第一条，新目录下第一个 session 会**挂在**信任提示上
  （不是弹个框就过去，是停住不动，而点名显示为「空闲」，见 `CrewSessionsSnapshot.state` 注释）。
- **删了会漏掉什么（诚实的那一栏）**: 旧 slug 下会永久留一堆不再对应任何真实目录的文件夹
  —— **整洁问题，不是正确性问题**，而且今天本来就有 62 个 worktree 的日志是这个状态
  （其中 52 个 worktree 已被删除）。万一将来 claude 改成按目录找，这套又需要；但风险可观测：
  新的降级路径会当场把 claude 的原话报进群里，不会静默失忆。
  `sessionsBusy` 建议保留，但注释要改 —— 它从「保护正在写的文件」降级成一条常识判断。
- **为什么当时没顺手删**: 那一版（Todo #68）改的是「续不上」的**病根**，属修复；删搬运是
  **清理**，风险面不同，diff 会盖过修复本身。**所以单独排了机长作战板 #12。**
- **已完成（2026-08-26）**: 照 §3.2 逐条删净 —— 两条 `Action`、五条 `Skip`、
  `Plan` 的四个输出面、整个清扫模式（`sourceDirectory` 回落 `previousWorkingDirectory`
  那支），连带 `Inputs.agentSessions` / `AgentSessionInput` 及其上游
  （`WorkdirChangeCommand` 里那次 `LocalAgentSessionStore.list()` 与 `memberName`、
  `LocalCrewStore` 的 `previousWorkingDirectory` 透传）、执行层两条动作与回执/预览渲染、
  界面预览两行。**§3.1 那四样一个字没动。**
- **两处只改语义不改行为**: `sessionsBusy` 保留、注释改了（它不再保护任何文件，
  从硬约束降级成常识判断）；`LocalCrewStore.previousWorkingDirectory` **字段保留**
  （持久化留痕，删它会把已写进 `local-crews.json` 的历史一次丢掉），注释写明
  **当前无消费者**。
- **评估漏了一处，删的时候才发现**: 机长 `change_workdir` 的**工具描述**里写着
  「留待清扫 / 幂等 / 再调一次」，还有第三个测试文件 `McpServerWorkdirToolTests`
  钉着那两个词。**漏的原因是评估按实现词（`transcript`）grep，而那几句里一个
  `transcript` 都没有** —— 这一刀的**对外文案面比代码面散**。已改口为「一次做完、
  没有第二趟」并把依据（`--resume` 按会话号找全盘）写进描述：**留旧文案比留死代码糟，
  死代码不骗人，过期的工具描述会让机长照着再调一次、以为自己补上了什么。**
  收尾时按语义（清扫/幂等/留待/再调一次）又扫了一遍，没有第四处。
- **删了会漏掉什么，仍然照上面那栏算数** —— 旧 slug 下会留着一堆不再对应任何真实
  目录的文件夹，是整洁问题不是正确性问题。

### 🟡 终端镜像视图里跨行拖选 + ⌘C 复制不出正确文本
- **发现**: 2026-09-02 · 人类在 P4 的 A2 测试包（0.1.23 build 2026090201，P4@77b996a）上实测，
  原话「选中文字还是不行」。2026-09-04 由父机长明确**拆成独立终端 bug 记账**，
  不再作为「常驻后台 / 前后端分离」P4→main 的合并闸门。
- **位置（未定位到行，只框到面）**: `Sources/Mac/LocalRunner/TerminalMirrorView.swift`
  —— 终端劈成两半之后（`c169fd5`），窗口侧是一个只吃字节流的 SwiftTerm 镜像视图，
  选中/复制由这个控件自己提供。设计文档 §2.4 当初的判断是
  「选中复制、回滚缓冲、reflow 全部由原生真终端控件提供，代码路径一行不改」——
  **这条判断被这次实测证伪了，但证伪到哪一层还没查。**
- **还不知道的（别当成已知）**: ① 只在 daemon/镜像路径出现，还是 inproc 老路也一样；
  ② 是选区几何算错、还是取文本时按行截断；③ 中文宽字符是否是触发条件之一。
  人类只回了「还不行」，**没有逐项现象**，所以上面三条一条都没有排除。
- **为什么没有自动测试照出来**: 选中与复制走 AppKit 的鼠标事件与 pasteboard，
  现有 A2 自动化（`TerminalMirrorParityTests` 等）验的是**缓冲区内容一致**，
  不是**选区→pasteboard 这条链**。这两件别混。
- **该怎么还**: 先在**不开窗口**的前提下把能验的那半验掉——直接对 SwiftTerm 的
  `Terminal`/选区 API 做单测：跨三行构造一段中英文混排的缓冲区、取选区文本、
  断言换行与宽字符。**先证明这把尺子会红**（用人类描述的那种形状），再修。
  只有确实非 GUI 不可的那一小段（真鼠标拖拽）才请人点一下。

### 🔴 「把文本从终端格子里弄出来」有三条各走各的路，各错各的
- **发现**: 2026-09-04 · P5a 真 daemon 冒烟。三条路是在同一天被三件不相干的事分别照出来的，
  这本身就是判据：**它们已经分叉到不会一起对、也不会一起错。**
- **三条路**:
  1. **AppKit 选中 / 剪贴板** —— `TerminalMirrorView`。人类实测跨行拖选 ⌘C 复制出来不对
     （2026-09-02，本文件另有一条独立记账；已从前后端分离主线摘出，别顺手一起改）。
  2. **无画面 `screenText`** —— `AgentSessionCore.screenText`（`inspect_session` 走它、
     写进白板的「它最后一句话」走它、`--daemon-attach` 探针也走它）。实测**本该是空格的
     位置输出 NUL**，而且不只在全角字后面：
     `'Claude\x00Code'`、`'auto\x00mode'` 全是纯 ASCII。病根是
     `line.translateToString(trimRight: true)` —— SwiftTerm 里没被写过的格 char 就是 `\0`，
     没人映射成空格。终端里 NUL 不显示，所以**肉眼一直看着是对的**，只有把它当文本用才露馅。
     修的时候要分清两种 NUL：没写过的格（`width == 1`）该变空格，全角字的后半格
     （`width == 0`）必须丢掉 —— 一视同仁会把「我在」变成「我 在」。
  3. **去 ANSI 的字节尾窗** —— `PendingDecisionTracker`（判「终端在等人选」）。
     claude 的「是否信任此文件夹」对话框在这条路上**认不出来**：序号后面没有空格、
     行尾 `\r\r\n` 又把连续的选项块切断。
- **为什么这是 🔴 而不是三条 🟡**: 三条路读的是同一份缓冲区、回答的是同一个问题
  （「屏幕上现在是什么字」），却各有各的解析。任何一条修对了都**不会**带动另外两条，
  而它们的错法互不相似（丢空格 / 认不出对话框 / 复制出错），所以没有一条测试能同时钉住。
  真正的债不是这三个 bug，是**没有一个「屏幕 → 文本」的单一实现**。
- **已经在做的**: 第 2 条正在修（映射成 U+0020，抽成共享纯函数 `TerminalScreenText`，
  让 `screenText` 与探针都调它）。**第 3 条与第 1 条明确不在那一期里**，各自独立。
- **该怎么还**: 等第 2 条那个共享纯函数落地后，把第 3 条改成读渲染后的画面
  （而不是自己去 ANSI 扫字节）—— 它要判的本来就是「屏幕上有没有一个选择菜单」，
  读画面比读字节流更接近问题本身。第 1 条走 AppKit，能不能并进来要单独评估，
  **不要假设它一定能**。


### 🟡 crew 白板通道会对某个 session **单向断开**，根因两次都没查出来
- **发现**: 2026-08-27（P4 交付期，见 `docs/internal/2026-08-27-p4-handoff-report.md` 第八节）
  与 **2026-09-04**（P5a 收口期）各一次。
- **症状**: 某个 session 的 `post_to_crew` / `read_whiteboard` / `directory` 全部报
  「未能打开文件 `<crewId>.json`，因为你没有查看它的权限」，**写入方向断、`report_to_parent`
  方向仍通**。工具回执明说「本次一个字都没写、原件未动」，所以不丢数据 —— 丢的是**话**。
- **已排除的**（2026-09-04 当场核过）: 不是文件坏了、也不是权限位不对 ——
  `ls -l` 是正常的 `-rw-r--r-- hey staff`，而且 **mtime 一直在跟着别的进程更新**
  （同一时刻别的 session 正常写同一个文件）。所以是**本进程被挡在那个目录外面**，
  不是文件的问题。
- **2026-09-04 新增的两个数据点**（比 08-27 那次多知道的）:
  1. **同一时间窗里有两个互不相干的 session 同时中招**（机长 + 一个 worker），
     所以不是某个 session 自己的状态问题。
  2. 它有一个**沉默的连带受害者**：`CrewMentionFilterRealWhiteboardTests` 那三条要读
     同一个目录，读不到就 `XCTSkip`。于是「通道断了」会伪装成「skip 从 3 变成 6」——
     **一次故障同时污染了沟通和判据**，而且两边都不报错。
- **代价**: 不是数据损坏，是**协作静默**。人和队友看不到你的话，而你自己以为发出去了
  （工具回执其实说了没发，但很容易被当成噪音划过去）。
- **绕法（两次都用了同一个）**: 结论写进仓库文件 + 走 `report_to_parent` 请上级代发。
  `docs/internal/2026-09-04-p5a-closed-loop-evidence.md` 就是这么落下来的。
- **2026-09-05 第四次：探针在断的当口抓到了，三条以前没有的事实。**
  （日志存在 `/tmp/pc-access-probe/<tag>.log`，脚本 `scripts/diag/whiteboard-access-probe.sh`）

  1. **写通、读不通。** 探针那行是 `write=FAIL(readback)` 而**不是** `FAIL(create)` ——
     建文件成功了，`head -c 1` 读回失败；`stat` 也通。**被拦的精确到只有 `file-read-data`。**
     以前一直记成「读写都拦」,那是因为没把建和读分开量。
  2. **探针脱离 claude 进程链之后照样断。** 它 `nohup` 之后责任链是 `sh(NNNNN) ← 1`,
     **不挂在任何 claude session 底下**,和同机的 claude session 一起断。
     → **「某个 claude session 自己的沙箱 profile」这条被证伪**（前几次包括我在内都往那儿找过）。
  3. **发作现场的 TCC 日志里,claude-code 正在逐个试受保护目录,而责任进程写的是 PendingCrew**:
     `AUTHREQ_ATTRIBUTION: responsible={com.pendingname.pendingcrew, pid=94648, /Applications/PendingCrew.app/…},`
     `accessing={com.anthropic.claude-code, …/claude/versions/2.1.261}, requesting={com.apple.sandboxd}`
     紧跟十二条 `System Policy: 2.1.261(NNNNN) deny(1) file-read-data …/Application Support/{AddressBook,
     CallHistoryDB, CloudDocs, Knowledge, MobileSync, com.apple.TCC, …}`,进程号递增、每条约 50ms。

     **这解释了「为什么日志里查不到我们那棵子树的拒绝」**（另一个 crew 翻了 12 小时日志确认过一条都没有）：
     **被记录的拒绝确实存在,只是路径是那十几个系统目录,不是我们的子树** —— 我们子树的失败
     **不产生独立的 deny 记录**。配上另一个 crew 量到的 tccd 反复报
     `Failed to match existing code requirement for subject com.pendingname.pendingcrew`（12 小时 14 次 / 7 个 service,
     同一 subject 同一 service 不同时刻答案还不一样）,一个能同时解释全部症状的机制是:
     **代码要求匹配失败 → 责任进程的授权拿不到 → 挂它名下的进程读不了那棵子树**。

  **⚠️ 上面最后那段是推的,别当结论**：那串扫描在 `11:00:23–26`,发作起于 `11:02:17`,
  **差了近 2 分钟,时间对不齐**。它可能只是同一周期性行为的另一次。判它要靠**两个 crew 同时挂探针**
  的同段日志对照（第一次具备这个条件就是这次）。
- **这次的时长**: `11:02:17` 起,到 `11:05:46` 仍未自愈（41 条「持续断」,≈3.5 分钟）。
  **「断多久」以前从没量到过** —— 前几次都是事后才发现已经好了。
- **断线期间 `contact` 的报错会说谎**：调 `contact <号码>` 得到的是
  **「查无此号 —— 本机没有这个 crew / 这个分机」**,而真实原因是**通讯录读不出来**。
  **它把「我读不到」说成了「它不存在」**,照这句去查的人会去核号码,而号码是对的。
  同一族的第 N 次（见本文件「静默失效的五种穿法」第 ⑤ 种：同一句话对应两种处境）。
  修法:读通讯录失败要和「号码不存在」分开报。
- **断线期间还能用的通道**: `report_to_parent` 通（写的是父 crew 的文件),
  `post_to_crew` / `plan_update` / `contact` 全断。**结论落仓库 + 请上级代发**仍是有效绕法。

- **该怎么查**: 下次发生时**当场**抓这三样（事后就没了）：① 本进程的 `sandbox` / TCC 状态；
  ② `ls -l@` 看扩展属性与 ACL（不只是权限位）；③ 同一时刻能写进去的那个进程是谁、
  它和中招的进程有什么不同。**别再只记「排除了什么」**——两次的排除项已经重合，
  再排一遍不会有新信息。

- **2026-09-05 第三次：边界量出来了，但我据此下的根因**（TCC）**当场被推翻了 ——
  两条都记在这，因为「被推翻」本身是这条目前最硬的信息。** 先说站得住的那一半：
  边界不是「某个文件」，是**整个数据根的文件内容**（这部分是量的）：

  | 探针 | 结果 |
  |---|---|
  | 列目录 `ls ~/Library/Application Support/PendingCrew/` | **可列** |
  | `stat` 取大小（元数据） | **可取**（112600 字节） |
  | `head -c 1 local-crews.json`（读内容） | **Operation not permitted** |
  | 在该目录下新建文件再读 | **失败** |
  | `~/.claude.json` / `~/.codex/config.toml` / 仓库文件 | **全部可读** |

  **形状是「元数据放行、内容拦截，读写都拦，且只拦这一棵子树」。** 这个形状本身是真的，
  它排除了 POSIX 权限和文件损坏 —— 但**推不出是谁在拦**：TCC 会长这样，一个沙箱层
  也会长这样。

- **⚠️ 我从这个形状推出「是 macOS TCC 保护 `~/Library/Application Support/<App>/`」，
  被两条证据当场推翻，别再照着它查：**
  1. **父 crew 机长同一时刻读写全通，而且责任进程是同一个** `PendingCrew(95591)`
     （他自己的链是 `zsh(18235) ← claude(7019) ← PendingCrew(95591)`）。
     同一个责任进程、几乎同一时刻、相反结果 → **不可能是挂在责任进程上的授权。**
  2. **它自己好了。** `02:41:53Z` 重探：`ls` OK / `stat` 112600 / `read` OK /
     `write` OK / 白板文件 OK，**中间我什么都没做**。
     **一个会自我恢复的东西，本来就不该往「授权被撤销」上想** —— 这是我推错的地方。

- **连带塌掉的还有我当时那条「关键证据」**：我拿「同一 session 内 `01:20Z` 读成功 →
  `01:43Z` 又成功 → `~01:50Z` 起全挂，中途没换进程」去论证「授权在会话中途丢了」。
  既然它随后又自己回来了，这段时间线**只能证明「有一段时间不通」**，证明不了授权丢失。
  **同一份观察，套上一个机制就变成了「证据」——机制是我加的，观察本身没这么说。**

- **前两次查不出来，现在有了解释：它是瞬时的、会自愈的。** 等有人去查的时候它已经好了，
  所以在文件那头怎么找都是干净的。**下次要抓就得在断的当口抓。**

- **同一次故障里还有第二个受害者，而且它也自愈了 —— 这一条收窄了范围。** 本 crew 的
  worker（`worker-2f960ba3`）在同一时间窗里 `post_to_crew` **连挂六次**，随后
  **不做任何事自己恢复**；而**父 crew 机长在同一时刻读写全通**。合起来：

  | | 状态 |
  |---|---|
  | 本 crew 机长（我） | 断 → 自愈 |
  | 本 crew worker | 断六次 → 自愈 |
  | 父 crew 机长 | 全程正常 |

  **同一个 crew 的两个 session 一起断、另一个 crew 的 session 不受影响。**
  这跟 2026-09-04 那次记的「两个互不相干的 session 同时中招」是同一形状 ——
  **两次都是「成对/成群地断」，不是某一个 session 倒霉。**

  所以「查那个 session 自己的沙箱 profile」这个方向**要打个折**：一个 session 独有的
  配置解释不了「两个 session 同时断、又同时好」。更像是**它们共用的那一层**
  （同 crew 的文件锁 / store / helper 进程），或者一个**短时的全局互斥**。
  但注意我这次读不了的不止本 crew 白板，`local-crews.json`（全局共享）也读不了 ——
  所以「按 crew 分隔」这个说法**本身还没被证明**，只是两个受害者恰好同 crew。
  **这句是推的，别当结论用**（上一轮我就是在这一步把推的写成了结论）。

- **下一步该查哪（父 crew 机长指的方向，我认）**: 差异在 **claude session 这一层**——
  两个 claude session，一个通一个不通，同机同责任进程同一时刻。查它们的**沙箱 profile /
  settings / 启动参数**的差异。**不是查文件（前两次），也不是查 PendingCrew 的 TCC（我这次）。**

- **连带**: 白板写不了的同时，**机长作战板（`plan_update`）也写不了** —— 它跟白板同在
  那棵子树下。所以症状不止「发不出话」，还有「进度板停更」，两者会被误当成两件事。

### 🟡 判断留在「接线层」= 只有「编译过」这一条保证（同一形状今天复发三次）
- **发现**: 2026-08-26（P4 编排闸门）与 **2026-09-04**（P5a 翻默认，同一天内两次）。
- **形状**: 一段判断挂在**进不了 test bundle 的接线上**（SwiftUI 视图、`Sources/Mac/Services`
  下的 client 对象）。它编得过、看起来也对，但**没有任何测试能证明它对**，
  于是错法只能靠人读代码发现——而它每次看起来都毫无异样。
- **三次实例**:
  1. P4：编排闸门挂在 `MacRootView` 的 `.task` 上 → 「app 到底取没取锁」在 app 侧
     **无法被证明**。修法不是补一句取锁，是把闸门搬到 `PendingCrewEntry.main()`
     ——**换到能被测的地方**（见 `docs/internal/2026-08-27-p4-handoff-report.md` 第二节）。
  2. P5a：`ViewerSessionClient` 里留了两处拉起判断（「锁上写着有 daemon 就不拉」
     「上次拉的还活着就不再拉」），其中第二条是实现者自己发明的规则。
     **发明的规则比继承来的更需要测试**——没有人替它背过书。已下沉为 `DaemonLaunchPlan`。
  3. P5a：`openLink()` 把「socket 连上了」当成「握上手了」——`isConnected = true`
     紧跟在 `connect()` 之后，从不等 `daemonHello`。判据要的是**握手**，实现拿了个
     **更早、更弱、本端单方面就能达成**的信号顶替。后果是接受连接但不回话的 daemon
     会让降级判定永远不被调用（读代码读到的，未复现）。
- **为什么它总是复发**: 接线是「把已经想清楚的东西连起来」的地方，所以往里塞一个
  `if` 感觉上不像在做决策。**判断力和可测性的边界不重合，是这条债的本体。**
- **该怎么还（两条口径，都已在用）**:
  - **接线里不许留判断**。留了就往下挪到纯判定层，连同它的理由一起挪。
  - **判据换了地方，就要跟着换到能被测的地方**——否则只是把一个「读代码才发现得了的
    错误」换成另一个「读代码才发现得了的错误」。
- **挑信号的判准**（第 3 例逼出来的）：一个能代表「对端确实在」的信号，
  **必须只有对端真的回过话才可能出现**。`connect` 成功不满足，`把 hello 发出去了`
  也不满足——两者本端单方面就能达成。
- **挑落点的判准**（2026-09-04 又逼出来的一条，机长自己指错过一次）：说「收进一处」
  之后，必须再问一句 **「那一处到底包住了哪几步」**。`LocalCrewStore` 这条上，
  机长指的落点是「把锁收进 `persistToDisk`」——听起来就是标准答案，实际上
  **锁内只有「写」**：12 个方法都是「先改内存数组、再调 `persistToDisk`」，
  「改」在锁外，「读」更是在启动时就发生了。正确的收口点是
  `mutate { crews in … }`（**锁内重读 → 应用变更 → 落盘**），调用方只交出「改什么」。
  **一个听起来就是标准答案的落点，可能只包住了三步里的一步。**

### 🟡 「期望状态没达成」被编码成成功：daemon 与 `--daemon-status` 都 exit 0
- **发现**: 2026-09-04 · P5a 两次真机实测各照出一半。
- **两处**:
  1. **daemon 本体**：`SessionDaemonMain.run` 的 `exit(error is StartError ? 0 : 1)`。
     于是「锁被**另一个 daemon** 占着」（期望状态**已经成立**，exit 0 是对的）与
     「锁文件**打不开**」（期望状态**没达成**，exit 0 是谎报成功）**共用退出码 0**。
     实测：数据根 `chmod 500` → `打不开 …/orchestrator.lock：Permission denied` → **退出码 0**。
     **任何拉起方都无法从退出码分辨这两者**（这也是为什么 app 侧的判据刻意不解析退出码，
     改由锁的观测去分辨 —— 见 `OrchestrationFallback`）。
  2. **`--daemon-status`**：`SessionDaemonStatusMain.runIfRequested` 的 catch 里只
     `print(...)`，不设退出码。实测（对一个只 accept 不回话的哑监听）：
     `PendingCrew 后台未运行或不可连接：后台已占用 socket，但状态握手超时` → **退出码 0**。
     一个诊断命令**说了「不可连接」却报成功**，脚本化用法分不出「后台好着呢」和「连不上」。
- **它是怎么被发现的**（值得照抄的做法）：worker 在跑之前先把预测写下来（「应当以人话报错
  **并非 0 退出**」）。行为那半对上了，**退出码那半没对上** —— 而如果没有事先写下预测，
  那句措辞漂亮的错误信息会让人直接点头过去，`退出码=0` 根本不会被看见。
- **该怎么还**: 两处合成一笔：把「期望状态没达成」一律映射成非 0；daemon 保留
  「锁被别的 daemon 占着 → exit 0」这一支（它确实是正确结局），其余失败非 0。
  **要先证明尺子会红**：现有的两条真机复现（`chmod 500` 的数据根、哑监听）就是现成语料。
- **不要顺手做的事**: 别让 app 侧判据回头去解析退出码 —— 那条路今天是故意不走的。
- **✅ 已还（2026-09-04）**：`DaemonExitCode`（`SessionDaemonHost.swift` 尾部，纯判定 +
  `DaemonExitCodeTests`）。改后实测（**上面那两份 `退出码=0` 是当时的历史记录，不改**）：

  | 情形 | 改前 | 改后 |
  |---|---|---|
  | 数据根 `chmod 500` 起 daemon | 0 | **1** |
  | 后台没在跑时 `--daemon-status` | 0 | **1** |
  | 已有**另一个 daemon** 时再起一个 | 0 | **0**（期望状态已成立，不变） |
  | 后台正常在跑时 `--daemon-status` | 0 | **0**（不变） |

  顺带把 `StartError` 拆开了：`alreadyOrchestrated(_, holderIsDaemon:)` 与
  `lockUnavailable` —— 「已经有人在编排」和「连锁都打不开」原来挤在同一个 case 里，
  退出码分不开正是因为**语义先分不开**。
  **这笔改不构成任何契约**：`DaemonExitCode` 的类型注释里写死了「判据一律不许读退出码」
  及其因果（退出码是约定不是观测，两种情况本来就可能同码），免得将来有人看到
  「退出码变准了」就把 app 侧判据改回去解析它。

### 🟢 daemon 不记「排空了哪几条机长命令」，事后无法判定重复执行
- **发现**: 2026-09-04 · P5a 收尾闭环。投了两条 `start_session`（claude + codex），
  `--daemon-status` 报出**三个** session，其中两个 claude 的 brief **逐字相同**。
- **判定不了**: 命令文件是「排空即删」，daemon 日志**只记启动/连接/退出，不记排空**。
  等发现异常时，文件没了、日志里没有，**证据在事发那一刻就已经不存在了**。
- **受控复现（做了，结论是「没复现」）**: 起 daemon **之前**写好**恰好一个**命令文件、
  `ls | grep -c crewcmd` 确认 = 1，再起 daemon → 结果 `运行中 session：1`，等 15 秒
  再问仍是 1，且那条 session 的开场 brief 正常送达并有回答。**命令通道重复执行没有复现。**
- **所以这条账记的不是那个异常，是那个异常查不了**: 最可能的解释是发起侧（一个 shell
  循环）多写了一个文件，但**我给不出证据**，只能说「没复现」，不能说「不会发生」。
- **该怎么还（很便宜）**: 排空时往 daemon 日志写一行 —— 命令 id、kind、crewId。
  一行日志就能把这类问题从「查不了」变成「一眼看出是一条还是两条」。
  **同类价值**：`start_session` 是要花订阅额度的动作，重复执行的代价不只是多一个进程。
  **那一行要连命令**文件名**一起记**，不只是 id/kind/crewId —— 两个候选解释在日志里
  长得不一样：「发起侧多写了一个文件」是**两条不同文件名**，「同一个文件被排空两次」是
  **同一个文件名出现两次**。只记命令 id 的话，若重复写入用的是不同 uuid，**两种解释仍然
  分不开** —— 那这行日志就白记了。

**⚠️ 2026-09-04 稍后：复现了，上面那条「最可能是发起侧多写了一个文件」是错的。**
（原文保留不改 —— 账本回头改数字就不是账本了。）**这一趟输入是数过的**：
`起 daemon 前命令文件数 = 2` → `运行中 session：3`，两个 claude 的 brief 逐字相同，
**agent 会话号是两个不同的**（真起了两个进程，不是显示重复）。三次实验形状一致：
**1 条命令 → 1 个；2 条命令 → 3 个，重复的是排在前面的那条。**

**病根**（读代码，且解释了全部三次观测）：`CrewStore.drainPendingCommands` 在循环里
**逐条 `sessionSpawnRequests.append(...)`**，而它是 `@Published`；消费方
`SessionHost` `.receive(on: .main)` 拿到的是**每次 append 各发一次的快照**：
append cmd1 发 `[cmd1]`、append cmd2 发 `[cmd1, cmd2]`。两次投递都到主线程：
第一次处理 `[cmd1]` 并清空，第二次拿到的仍是发出时那份 `[cmd1, cmd2]` →
**cmd1 被处理第二遍**。

**形状本身错了**：发布的是**变化**，消费的却当成**待办队列**。别用「按 id 去重」
糊 —— 那是在下游擦屁股。`CrewStore` 里注释写着「数组语义同 `sessionSpawnRequests`」
的还有几条，**同一形状要一起看**。

**代价**：`start_session` 花订阅额度，机长连派两个活时第一个会起两遍。
**不是 P5a 回归** —— GUI 模式同一份代码，应该一直都在。

**给下一个人的那条教训**：我第一次查不下去时给了一个「听起来合理的归因」
（发起侧多写了一个文件），差点把这个 bug 结掉。**给不出证据时只说「查不了」，
别给归因** —— 一个合理的归因比一句「不知道」更能让人停止追查。
**更锋利的说法（worker 提的，比上面那句好）**：「最可能是**我自己**错了」听起来是谦虚，
其实和「最可能是**它**的错」一样是**归因**，只是方向朝内 —— 两者一样会把问题结掉，
**而朝内那种更难被人反驳**。当时唯一站得住的只有「查不了」。

### 🔴 默认值要落在「失败看得见」那一侧，不是「失败最少」那一侧

- **发现**: 2026-09-05 · 给半开连接回收加开关时（`ddd7d25`），父机长要求把这段推理
  单独立账 —— 它比那次改动本身更通用。
- **规矩**: 一个开关的默认值，**别按「哪一侧出错概率低」选，按「哪一侧出错会喊」选。**
- **本例**: `SessionProtocolServer(reclaimsIdleConnections:)`
  - 默认 `false`（不回收）：忘了填 → 半开连接堆着 → `--daemon-status` 数得出来
    → **看得见的缺功能**。
  - 默认 `true`（回收）：忘了关 → 同进程桥的连接**60 秒后被静默打死**，
    而它的 app 侧根本不发 ping → 症状是「session 莫名其妙掉线」，
    **跟我们本来要修的那个 bug 长得一模一样**。
  - 两种遗漏的概率差不多，**但一种会喊，一种不会**。选会喊的那一侧。
- **它和「静默失效」是同一件事的两面**：下面那条讲的是**运行期**怎么别把失败吞掉；
  这条讲的是**设计期** —— **在选默认值的那一刻就把沉默的那一侧关掉**，
  因为沉默的错误比吵闹的错误贵得多，而默认值决定了「大多数人会落在哪一侧」。
- **配套**：同一次改动里也记下了**这个开关挡不住什么** —— 有人**故意**给同进程桥
  传 `true`。拦它要把桥的 `private let server` 暴露给测试，为一次蓄意误用放宽封装
  不划算。**「挡住了『忘了』，剩下只有『故意』」这句话本身就是交付物的一部分**：
  要的不是覆盖率数字，是知道自己盖住了什么、没盖住什么。

### 🔴 「静默失效」的五种穿法 —— 这一期反复撞的其实是同一件事
- **发现**: 2026-09-04 · P5a 全程。**不是五个 bug，是同一类毛病的五种穿法**。
  之所以值得单列：修掉其中一种**完全不会**让另外四种露出来，而它们的症状互不相似，
  所以每一次都像是「又一个新问题」。
- **五种，各配当天的真实实例**:
  1. **不报错** —— 开场 brief 被首屏重绘吞掉，TUI 起来了、输入框空的、transcript 零条，
     状态照样 `.running`，点名显示「空闲」。
  2. **报了，但那句话不可操作** —— 卡住的 daemon 持着锁不回话，界面一直说
     「后台进程正在运行，本窗口继续重连」。措辞越让人安心越难被发现。
  3. **报成功，其实失败** —— daemon 拿不到锁时打一行原因然后 `exit(0)`；
     `--daemon-status` 说「不可连接」也 `exit 0`。**把「期望状态没达成」编码成了成功。**
  4. **无限期报「正在进行」** —— `openLink` 把「socket 连上」当「握上手」，
     于是接受连接但不回话的对端让降级判定永不被调用，界面在「重连中」里打转。
  5. **同一句话对应两种完全不同的处境** —— `--daemon-status` 的输出里**一行都没提数据根**，
     于是「后台确实没在跑」和「你问错了数据根」长得**一模一样**；
     失败那条同样不说它去哪儿找的。
- **它们的共同解药**（三条，都在这一期用过，按强度排）:
  - **把「我在看哪儿 / 我凭什么这么说」写进输出**（第 5 种、那行排空日志都是这一条）。
  - **让错的那种写法表达不出来**（`Bool` → 三态；12 个落盘点 → 一个收口；
     入队与发脉冲绑进同一个 `enqueue`）。
  - **给「等待」一个寿命** —— 任何「正在……」都必须有上限，超时后**文案升级成可操作的**
     （第 2、4 种）。
- **⚠️ 三条解药不能互换，别拿错工具**（2026-09-04 补，worker 提）:
  - 「**让错的写法表达不出来**」只在**错的形状进得了类型系统**时管用 ——
    `Bool` 换三态 / 落盘收一个口 / 入队与发脉冲绑死，都是因为错法能被编译器表达成
    「一个可以填错的参数」或「一个可以漏掉的调用点」。而**第 5 种（同一句话对应两种
    处境）没有这种把手**：没有哪个类型拦得住「这句话说得不够」。
  - 反过来，「**把我在看哪儿写出来**」对第 2、4 种**不够** —— 无限期「正在连接」
    当时说的话**没有错**，错的是它**永远不变**。
  - 粗略对应：**写出依据 → 第 3、5 种；让错法写不出来 → 第 1、2 种；给等待寿命 → 第 4 种。**
    三条一起才盖住这一族。
  - **给等待寿命有个反面**：寿命给短了会把「慢但没事」变成**新的假警报**。所以超时文案
    必须连「**等了多久、我据此认为什么**」一起说 —— 这又绕回第一条。
    **这一族的解药里，「写出依据」是地基。**
- **诊断时的第一步（零代码，现在就能用）**: 看到「后台未运行」**先别判故障**，
  先确认问的是不是**同一个数据根**（带上 `PENDINGCREW_DATA_DIR` 重问一次）；
  仍说没在跑，再看 `orchestrator.lock` 在谁手上 —— **锁是独立于 socket 的第二个观测量**，
  它能把「后台没了」和「socket 连不上」分开。
- **第 5 种为什么当天没修**: 那行输出是**验收路径上的诊断工具**，
  在验收进行中改它等于**把人正在照着用的尺子换掉**。记账，等验收有结论再动。

### 🔴 白板「单向断开」：范围从来不是我们那棵子树 —— 是整个 `Application Support`
- **发现**: 2026-09-05 · 第四次发作，**第一次抓在现场**（机长 4-1 自己中招，
  03:02–03:06Z 持续 4 分钟以上）。
- **前三次为什么查不出来**: 结论一直是「只拦 `~/Library/Application Support/PendingCrew/`
  这一棵子树」。**那是探针清单造成的假象** —— 对照组试的是 `~/.claude.json`、
  `~/.codex/config.toml`、仓库文件，**全都在 `Application Support` 之外**。
  范围被划在了刚看过的东西上，于是它看起来就像主因。
  **一测别人的目录就露馅**：
  ```
  Application Support/Code/Backups     → EPERM
  Application Support/Claude/Cache     → EPERM
  Application Support/PendingCrew/**   → EPERM（含当场 touch 出来的新文件）
  ```
- **精确形状（现场实测，不是推的）**:
  ```
  ls 目录 → OK          stat 元数据 → OK          touch 建文件 → OK
  读任何文件内容 → EPERM(1)
  python3 open().read(1) → OSError errno=1        head / dd → 同样一句
  ACL → 无     flags → -     log show 近 3 分钟 deny/Sandbox → 一条都没有
  责任链 → zsh ← claude ← PendingCrew(daemon) ← PendingCrew
  在跑的二进制 → 只有 /Applications 那一份（9 进程），/tmp 测试包 0 个
  ```
- **被这次现场打掉的两条假说**（都曾是当时的主方向）:
  1. **「是 agent 权限层在假冒内核的措辞」** —— `python3` 原始 syscall 就是
     `errno=1`，三条读法一致。**不是工具层。**
  2. **「同 bundle id 两份不同签名同时在跑」** —— 这次发作时 `/tmp` 那份**一个进程都没有**。
     它当不了**现场原因**（但仍可能是**历史原因**，见下）。
- **当前最像的一条（推的，未验）**: 读 `~/Library/Application Support/**` 属于要
  「完全磁盘访问 / App 数据」授权那一类，授权挂在**责任进程**上 —— 我们的责任进程是
  PendingCrew。它的**代码签名对不上 TCC 里那条记录**的那一刻，授权失效，其全部后代
  （claude → bash）读整个 `Application Support` 变 EPERM；对上了就恢复 = 自愈。
- **⚠️ 尺子拿错了三次，这条比结论更值钱**: 一直在 grep `deny file-read-data`，
  **这一类拒绝不长那样**。真正的痕迹是 `tccd: Failed to match existing code requirement
  for subject com.pendingname.pendingcrew and service …` —— 12 小时 14 条、横跨 7 个
  service，**同一 subject 同一 service 不同时刻答案还不一样**。
  「发作窗内没有拒绝日志」这个让人困惑了三次的事实，**是因为在拿错的尺子上找**。
- **探针必须改的三处**:
  1. 对照组**必须包含别的 app 的 `Application Support` 子目录**，否则永远得出「只拦我们这棵」。
  2. 抓 `tccd` 的 `Failed to match existing code requirement`，别只抓 `deny`。
  3. 每次记下那一刻 `/Applications` 与 `/tmp` 两份二进制**各有几个进程**。
- **不要自己修**: `tccutil reset` 会清掉人类真实授予过的权限，属人类授权，不是 agent 能拍的。

## `wakeBusyStallAlert` 本机全历史 0 次触发 —— 未区分「没发生」与「触发不了」（2026-09-12 排掉一半）

`CrewMailboxWakeLogic.wakeBusyStallAlert`（`confirmWake` 判失败时、目标仍挂着忙碌
指示的那一支）在全机 47 个白板的**全部历史里一次都没有出现过**（2026-09-07 实测；
同一次统计里另一支 `wakeFailureAlert` 有 112 次，所以不是统计口径的问题）。

**「从没触发过」有两个完全不同的意思，这个读数分不开它们**：

1. 它防的情况真的没发生过 —— 代码是对的，只是没派上用场；
2. 它**根本触发不了** —— 判据（`wakeTargetLooksBusy` / `shouldKeepWaiting`）写错了，
   永远进不去这一支。

一个从不发声的东西看起来像「没问题」，实际可能是「已经退出检测器行列了，只是
没人宣布」。**这正是「先证明尺子会红」那条纪律的对象，只不过这次的尺子是一段告警代码。**

**不删、不改**（删掉就等于替它选了解释 ①）。**要区分只能靠一次构造实验**：人为造一个
「挂着忙碌指示但既无新输出也无新发言」的目标，看它响不响。响 = ①，不响 = ②。

出处：人类 Todo #105 ④ 完工复核，4-1 拍板单独记账。

### 构造实验做了，② 只排掉了一半（2026-09-12）

**先修了一件更基础的事**：那个「选哪一句」的判断原来是 `CrewSessionRunner.confirmWake`
里的一个三元表达式，而那个文件不进 test bundle —— **所以「busy 那一支选不选得中」
在这个仓库里根本没有尺子量得到**。判据一个字没改，只把它搬进
`CrewMailboxWakeLogic.unconfirmedAlert`，让它站在量得到的地方。

然后是四条断言（`CrewMailboxWakeLogicTests` 末尾那一组）：挂着忙碌指示时选得中；
什么都没挂（含一拍没采到）时选的是老那句；**忙碌指示本身不算到达证据**；
挂着指示也等得到寿命上限、不会无限等下去。

**关键的是第三条**：`isBusyNow` 一旦被加进 `receiptVerdict` 的或运算，挂着指示的
目标会先被判成 confirmed，busy 那一支**从此永远选不中** —— 正好变成解释 ②，
而且盘上一点痕迹都不会留（它本来就是 0 次，谁也看不出少了什么）。
两刀变异各自证过：加进去 → 第三条红；`unconfirmedAlert` 恒回老那句 → 第一条红；
还原后 19 条全绿。

**所以现在能说的只有**：在**纯判定这一层**，② 不成立。

**仍然不能说的**（别把这一组读成「确认是 ①」）：真实世界里 `isBusyNow` 能不能在
`lastOutputAt` 一动不动的同时挂满 300 秒。claude 那条路上 `isBusyNow` 读的是终端
状态行，而状态行会**停在屏幕上**，所以看起来可能；但这要一次真实现场，单测量不到。
这条账因此**不销**，只是缩小到了那半。

## B3 只治「看得见的丢」——`pinPosition` 返回 `.retryLater` 那条路磁盘上零痕迹（2026-09-12：不再零痕迹，但仍然丢）

2026-09-07 修的 B3（启动积压对账，`CrewStartupRescueLogic`）治的是**能从白板上看出来的**那半：
重启前没被处理掉的定向 @，现在启动时会按每个成员自己的盘上未读对一次账、补投。

**另一条真的丢法完全没有痕迹，这次修复没碰它，也不该被认为一起解决了**：

`CrewLocalMentionWaker.pin` 在白板读失败时走 `CrewLocalMentionWakeLogic.pinPosition`
的 `.retryLater` 分支 —— 那个 crew 这次不钉，留到下一次白板事件再钉，**而下一次钉的
仍然是「当时的尾巴」**。中间写进来的 @ 落在游标前面，谁也扫不到。

**它在磁盘上不留任何东西**：没有告警、没有计数、白板上看不出「这里本该有人被叫醒」。
所以：

- **量不到。** 2026-09-07 那次统计（三次重启各 14/12/13 个 crew 暴露）只覆盖得到
  「白板最后一条是没人回过的定向 @」这种**看得见**的形状。
- **也不打算假装量得到。** 要拿到它只能造实验：人为让某个 crew 的白板读失败，
  在窗口里写一条 @，看它有没有被扫到。

⚠️ 同一份统计里那句边界要一起带着：**「三次重启各 14/12/13 个」是暴露面，不是
受害者名单** —— 板死可能有别的原因。只有 crew 33 是硬的（人类的 Todo 答复写在
重启前 38 秒、目标在本地成员登记里、此后 33 小时白板全空），crew 45 次硬。
**重启标记 2026-09-04 才有，所以那三次是下限。**

出处：人类 Todo #105 的 B 组残留，4-1 拍板单独记账。

### 2026-09-12：**补不回来，但让它留痕**

先把「还能不能补回来」问到底，答案是**不能**，理由是硬的：读失败时
`LocalWhiteboardStore.loadLocked` 返回的是 `[readFailureWarning(error)]` ——
**整份只有一条内存里的警示行，没有任何真实历史行**。所以「退一步钉在最后一条
真行上」这条看似显然的修法**根本不存在锚点**。（顺带校正一处旧注释：
`CrewLocalMentionWakePinTests.testPinSkipsWhenSyntheticRowIsTheTail` 那条造的是
`[history, 警示行]`，注释说这是「白板重建后的形态」—— 不是。重建走
`rebuildWithWarningLocked`，它写的那行**是新 UUID、而且真落盘**，返回的也只有
它自己一条。那条测试守的是一个 store 产不出来的形状，留着无害，但别照它推断现实。）

所以改的是**代价的形状**，不是代价本身：补钉成功的那一刻白板已经可读了，
就在那儿留一行，写清窗口两端（`CrewLocalMentionWakeLogic.missedPinWindowNotice`）。
**丢还是丢，但从「静默地丢」变成「板上写着几点到几点之间的定向 @ 没被扫到」**——
人回头查得到，也终于量得到。

三条守着它：有缺口时写清两端和时长；没缺口时**一个字都不写**（一个「永远留一行」
的实现会让第一条照样绿，而它会把白板灌成噪音）；留痕那一行**谁也叫不醒**
（无 mention 的 session 条目在 `pending` 里返回空 —— 把这个前提钉住，免得哪天
有人给系统条目加「默认 @机长」，让留痕变成每次都吵醒机长）。三刀变异各自证过会红。

**留痕本身也可能写失败**（白板不可读的那一刻，写未必就通），所以记录是**写成功
才清**，没清掉的下一次 `scan` 再试一次 —— 否则这条留痕会以「它自己也静默失败」
的方式复现它要治的那个病。

**仍然没做的**：那条构造实验（人为让某个 crew 的白板读失败、在窗口里写一条 @、
看它有没有被扫到）**还是没做**。这次做的是留痕，不是验证丢法。

## ~~发版闸门的 xcresult 没被归档~~（2026-09-12 已修）——**留得住日志的是日常跑，留不住的恰恰是最要紧那一趟**

2026-09-07 落了 `scripts/test-mac.sh`：跑完把 xcresult + 完整日志归档到
`.test-archive/`（留最近 10 趟）。**但它只覆盖日常跑。**

**`scripts/release-gate.sh` 仍然只留 `.log`、不留 xcresult。**

### 为什么这个缺口比听起来贵

**日常跑红了可以再跑一次；发版闸门红了、日志又丢了，你面对的是一个已经开始的
发布流程和一条查不出来的红。**

出处不是假想：2026-09-07 有人在**开发跑**里撞过一次 —— 一趟全量报 `2 failures`，
只 grep 了汇总行，回头去查时 Xcode 已经把 DerivedData 里的 xcresult 轮转掉了，
**失败用例名永远拿不到**。那条红最后只能记成「一次未复现的红 —— 不是『飘』，
也不是『已修』」。**那还只是开发跑。**

### 为什么没顺手补（拒绝的理由，别当疏忽）

`release-gate.sh` 不指定 `-derivedDataPath`，产物落在**共享** DerivedData 里。
多条线并跑时，「最近那个 xcresult」是谁的**并不确定** —— 在那儿猜一个反而会
**归档错东西**，而那比不归档更坏：**它会让人拿着别人的日志去查自己的红。**

### 修法的前置

**先给 `release-gate.sh` 一个独立的 `derivedDataPath`**，再照 `test-mac.sh` 的做法
归档。那是动发布路径，**单独一笔**，不要在别的改动里顺手做。

出处：人类 Todo #105 收尾，4-1 拍板单独记账。

### 怎么修的（2026-09-12）

按上面写好的前置来：先给 `release-gate.sh` 一个**显式的 `-derivedDataPath`**
（`/tmp/pcw-<commit>-log/dd`，三趟 xcodebuild 共用），再照 `scripts/test-mac.sh`
的形状把 xcresult 拷成 `<归档时刻>.xcresult`。

**「猜最近那个 xcresult」这件事没有被做得更聪明，是被取消了** —— 目录一旦是这一趟
自己的，里面的 xcresult 就只可能是它自己的，不存在拿错别人日志的可能。

两个位置是刻意的，别顺手挪：

- **DD 放 `$LOG` 底下，不放 worktree 底下。** worktree 里多一个未跟踪目录会被
  `status --porcelain -uall` 数进去，读数 ④ 的前后指纹就必不相等 ——
  `.test-data-root` 2026-09-09 正是这么把 ④ 弄成过报的。
- **归档紧跟在 `test` 后面，在两趟 build 之前。** build 挂住或被人打断时，
  失败用例名已经落盘了。

验的方式：把脚本里那段归档块**原样抠出来**跑三种输入（没有 xcresult / 有 /
同一个 commit 复跑），分别拿到「警告且归档为空」「拷进去了且报的是真路径」
「两份并存不覆盖」。第一版harness 忘了 export，阴性那条是**因为变量是空串**
才绿的 —— 「先证明尺子会红」这一步当场救了一次。

**没做的那半，说清楚**：那个 DD 是这道闸门最大的一块占地（GB 级），脚本
**仍然不自动回收**，只在末尾多报一行说明大头是它。要不要清、什么时候清，
按这个脚本一贯的立场，是仓库主人的事。

## 检测器看不见的 4 条裸引用 —— 记账，不是待办

`scripts/doc-ref-check.sh` 只认「路径第一段是仓库根下真实存在的条目」的引用。
按这个口径全库共 114 处裸引用（形如 `Name.ext:N`、路径里没有 `/`）：105 处今天就成立、
已补上路径（`9585514`），5 处只是没声明基准提交、已补 base（`d531734`），
**剩下这 4 处裸名本身判不了**，逐条记在这儿：

| 出处 | 引用 | 是什么 |
| --- | --- | --- |
| `docs/internal/2026-08-10-pendingcrew-ios-driving-channel-design.md` 第 225、504 行 | `LocalWhiteboardStore.appendRelayMessage:197` / `:192` | **不是「文件:行」，是符号引用**（`类型.方法:行号`）。检测器天生看不见它（没有 `/`），这次盘点的正则把它捞进来了而已 |
| 同上，第 503 行 | `CrewRelayAgent.swift:162` | 指向**已从仓库删掉**的文件（那份文档写的是当时的树） |
| `docs/internal/2026-09-07-chat-message-folding.md` 第 92 行 | `BlockNode+View.swift:16` | **第三方包里的文件**（`swift-markdown-ui`），不在本仓库，所以找不到是对的 |

**为什么不动它们**：

- 前三条属于「那份文档描述的是过去某一刻的树」，处置方式和组②一样是补基准提交，
  不是改行号；但它们是裸名，补基准也无从校验，留着比改错好。
- 最后一条是**另一类病**：引用没写清它在哪棵树上，只不过那棵树不是我们的仓库。
  按 `CONTRIBUTING.md` 新增的那条约定，应写成「`swift-markdown-ui` 某版本的
  `Sources/MarkdownUI/Views/Blocks/BlockNode+View.swift` 第 16 行」。
  **别为它去扩检测器** —— 要判它就得解析每个依赖的版本、拉源码再数行，
  那是把零判断的尺子变成有判断的，然后它会开始误报、被人关掉。

出处：机长作战板 #16 清理批次，组④。上级明确清理不含这 4 条。

## ~~「顶层机组向上汇报落到总机组」没有端到端验过~~（2026-09-12 已销）

人类 Todo #141 / #137 做的是：总机组永远**不作为一条存下来的父边**存在
（`refuseBuiltin` 那四道一个字没动），「顶层机组向上汇报落到它」改成派生
（`LocalCrewStore.reportingParentIds`，判据在 `CrewReportingParent.resolve`）。

**验到的**：派生规则本身（`ReportingParentTests`，9 条，四处变异各自会红）；
规则有没有被接到投递路和注入路上（`ViewWiringTests` 两条接线断言 —— 补它们之前，
把那两处改回 `parentIds` 全量 2553 条**一条都不红**）。

**没验到的，就是这笔账**：**从来没有真的发出过一条汇报，去看它是不是落进了
总机组那本群聊。** 验的是「派生出来的 crew id 对不对」，不是「那条消息到了」。
中间还隔着 `LocalWhiteboardStore.appendSessionMessage` 和唤醒那一段。

怎么还：等人装上带这些代码的版本之后，在任意一个顶层机组里让机长
`report_to_parent` 发一条，然后看总机组的群聊里有没有那条、署名是不是
「<源 crew 名>·机长」。**一次手工验证就够**，不需要为它造自动化 ——
它要的是一个跑起来的 app 加一个真实的白板目录，做成自动化的代价远大于收益。

**为什么不是「那就现在验」**：当时装着的是 0.1.32，比这些代码早两小时四十分，
压根没有这条路可走。

出处：人类 Todo #141，父机长明确要求记账而不是只写在那次回报里。

---

### 2026-09-12：验过了，销账

人装上 0.1.34 之后，父机长在顶层机组 `PendingCrew` 里调了一次 `report_to_parent`。
读数：

- 发之前 `whiteboards/pendingcrew-chief.json` **不存在**（发之前先看过，这一步是
  为了让读数干净 —— 否则分不清「新写进去的」和「本来就有的」）；
- 发之后该文件**出现**，里面 3 条，其中那条署名「PendingCrew·机长」、
  `senderKind=session`。

**派生投递整条路通了**：`reportingParentIds` 派生出总机组 →
`appendSessionMessage` 落进它的白板 → 文件被懒创建。这正是这笔账要的那件事。

同一拍还带出一个真发现（另记）：另外两条是系统报错「自动拉起机长失败：这个 crew
没有工作目录」—— 总机组当时起不了机长。已在同一分支修掉（给它补工作目录）。

---

## ~~`quietLayouts` 恒读 0，两条防线因此是空绿~~（2026-09-12 发现，同日**证伪并修掉**）

**结论先说：那两条防线没有空绿，尺子也没坏。** 坏的只有一个标本。
下面把原样留着，是因为得出它的那条推理本身值得留（见末尾）。

### 当天真正量到的

给同一个 `quietLayouts` 喂一个**已知有毒**的标本，它当场读出 71690 / 64100 次。
交叉对照四格，**三格自激**：

| 标本 ＼ 骨架 | 聊天（`quietLayouts`） | Todo 面板（`quietTodoLayouts`） |
|---|---|---|
| `SwiftUIRepeatForeverDots` | **0** | 37574 |
| `TodoCircleAsCrashed` | 71690 | 39161 |

右上角说明**标本仍然有毒**，左下角说明**尺子仍然量得动**。
坏的是「这个标本 × 这个骨架」这一格，不是尺子，也不是那两条 `LessThan`。

那一格为什么坏没有钉死：延迟、`scaleEffect`、`.onAppear` 换 `.task`、固定尺寸 Shape
换 SF Symbol，八个变体全是 0，而能复现的标本与它相差不止一处。**到此为止，不写猜的成因。**

### 修法

把那条前提测试的标本换成仓库里**已经存在**的 `TodoCircleAsCrashed`（不新造标本），
并在它头上写明：**它同时是 `quietLayouts` 的阳性对照** —— 那两条 `LessThan` 一旦
因为尺子死掉而空绿，这条会先红。另加一条「老标本在这个骨架里已不自激」的观察测试，
老标本因此不会从仓库里消失，SwiftUI 哪天变回去它会红。

`LayoutLoopRegressionTests` 6 条 / 0 红。

### 留着这一节的理由

原判断是：「三次调用全读 0 ⇒ 尺子恒读 0 ⇒ 挂在它上面的两条 `LessThan` 是空绿」。
**三个前提为真，结论是假的** —— 因为那三次调用喂进去的三个标本，恰好都不自激。
判「尺子还活着」唯一的办法是**喂它一个已知会红的输入**，而我当时没喂，
改为从「它给出的读数都是 0」反推。

这正是 [[pick-the-ruler-for-the-risk]] 里那条「先证明尺子会红再信它的绿」——
当时把它用在了产品测试上，没用在**自己的诊断**上。
相关：[[a-check-that-never-fires]]、[[true-frame-covering-fact]]。

---

## ~~接线扫描会被一句注释满足~~（2026-09-12 发现，同日修掉）

`ViewWiringTests.wirings` 的判据是「这个符号在定义文件**之外**的 `Sources/` 里
出现过」。**出现在注释里也算。**

新接一个按钮时当场撞到：把按钮动作整个摘掉（`Task { }`），尺子照样绿 ——
因为那个 View 顶上写着一句「动作在 `CrewStore.requestChiefResort`」。
零件没装到车上，尺子说装了。

### 这个洞是量出来的，不是推出来的

拆掉 `TodoListPresentation.newestFirst` 在详细窗口里那个**唯一的真调用点**
（只留 `CrewTodoPanel` 顶上那句提到它的注释），跑两趟：

| | 结果 |
|---|---|
| 不剥注释（原样） | **绿** —— 洞是真的 |
| 剥掉注释 | 红 |

### 修法：用仓库里已有的那个孪生

本仓另外四份源码级扫描（`TodoBlockedOnHumanTests`、`DecisionKindHasNoProducerTests`、
`TodoDroppedAndAttentionTests`、`CockpitOpenCloseCostTests`）**早就各自带着
`codeOnly` 了** —— 其中一份的注释还写着它当初也是变异自证抓到的。
`ViewWiringTests` 是漏掉的那一个。照抄过来，17 条一次性全修好。

### 逐条复核的结果（先前这里写的是「约 30 条没复核」，两处都不对）

清单是 **17 条不是 30 条**（数过了），而且**没有一条是当下空绿的** ——
每一条在 `Sources/` 里都至少有一个真调用点。两条身上带着注释凑数：

| 条目 | 真调用点 | 注释命中 |
|---|---|---|
| `TodoListPresentation.newestFirst` | `CrewTodoDetailWindow.swift:154` | `CrewTodoPanel.swift:14` |
| `CrewMentionFilter.onlyHumanMentions` | `CrewTimelineFilter.swift:52` | 同文件 :25 |

这两条在修之前是「离空绿只差一次删除」：真调用点一旦没了，注释会接着顶住。
剥注释之后这层顶不住了。

**先前那句「其余约 30 条没有逐条复核」是估的，不是数的。** 记在这儿是因为
[[verify-the-list-not-just-its-items]] 说的就是这件事：报 N 条就把 N 个名字列全再数一遍。

## 「写之前要先读」的留痕，在整目录读不出来时必然写不进去 —— 这是一类，不是三处

2026-09-12 这一天撞了三次同一个形状，值得当成一类记下来，而不是三条各自的 bug：

| 哪儿 | 它本来要留的痕 | 读不出来时会怎样 |
| --- | --- | --- |
| `CrewLocalMentionWaker` 的「游标没钉上」窗口 | 哪段时间的定向 @ 扫不到 | 写不进去（已改成写成功才清记录，下次 `scan` 重试） |
| 三本账的 `reportIncident` | 是哪种事故、哪本账 | 写不进去（已改成写不成退系统日志） |
| `CaptainTodoSweep` 的那段提醒 | ——（它只是**引用**上面那条警示） | 把人支去找一条必然不存在的东西（已改口径） |

**共同机制**：`LocalWhiteboardStore` 的 append **要先把整份白板读一遍**，读不了就
`throw unreadableAndPreserved` 整条拒写（2026-08-12 P0 的不变式：读不出来 ≠ 内容
损坏，一个字节都不许动 —— 那条不变式是对的，不要为了留痕去动它）。

**所以判据是**：一个写，如果它的**唯一目的**是记录「出事了」，而**触发它的那件事
本身就会让白板读不出来**，那它就是在最需要它的时刻缺席的。这类必须用
`appendSessionMessageReportingFailure` + 退路（系统日志 / daemon 日志），
不能用吞错的那一支。

### 还剩多少没审（读数，不是待办）

改完之后全仓仍有 **44 处** 吞错版 `appendSessionMessage(`（`Sources/`，不含定义本身；
2026-09-12 数的）。**这 44 处没有被逐条分类** —— 参数换行，一次 grep 分不出哪些是
「纯留痕」哪些是普通消息，而我没有逐个打开看。**别把「只改了 3 处」读成「另外 44 处
都审过没事」。**

**也不建议一刀切全换**：普通群消息发不出去，其缺席本身就是可见的（人会问「怎么没
回」）；把 44 处全改成 try/catch 只会把噪音摊平，让真正要紧的那几条淹掉。要挑的是
上面那条判据命中的。

`reportIncident` 那一族已经有闸守着（`CaptainTodoSweepTests`，名单是扫出来的，
将来多一本账自动进闸）。其余 44 处**没有闸**。

## 两态 `list()` 还有 42 个调用点 —— 查过的只有承重的那一个（读数，不是待办）

「读失败被压成空表」这个病在本仓咬过两次（`plan_list` 说「任务列表是空的」而账有
92 条；`CrewSessionRunner` 里两处点名「必须走 `read` 不能走 `list`」）。三本账现在
都有三态读（`LocalTodoStore` / `CockpitPlanStore` / `LocalWakeupStore` 的 `LedgerRead`）。

**但两态的 `list(crewId:)` 还有 42 个调用点**（2026-09-12 数的，`Sources/` 下）。

**只逐条查了承重的那一个**：`McpPermissionHook` 拿 Todo 判 `hasPendingRequest`，
拿 `PermissionGrantStore.consume` 判 `hasGrant` —— 这是**放行/拒绝**的判据，
读失败压成空表会不会放行一个不该放的？

**不会，两个方向都 fail-closed**（读代码读到的，不是推的）：

- `consume` 读失败 → `rows` 空 → `contains` false → 返回 false → **拒**。
- `hasPendingRequest` 只在 `.denyWithoutFiling` 和 `.denyAndFile` 之间选，
  **两支都是拒**；读失败最多让它重复提一条人类 Todo，不会多放行。

代价只有一个，而且只在故障期间：人已经同意过的那一次，票读不出来 ⇒ 照样拒 ＋ 再提
一条，人会看到「我要跑 X」问第二遍。

**其余 41 处没有逐条看过** —— 从名字看多是渲染/展示路（侧栏末条、未读数、注入面），
那类压成空只是少显示，不会产生假的业务结论。**但「从名字看」不是「查过」，
别把这一条读成「另外 41 处都审过没事」。**

## 十个源码扫描类断言，只有一个的剥离口径会让它对字符串瞎（2026-09-12 查过）

今天在 `CaptainTodoSweepTests` 里亲手造出一个假绿：要判的是**字符串内容**，
筛文件却用了**剥掉字符串之后的文本**，于是关键词只出现在字符串里的文件一个都没被
选中。变异（把一处改回旧话）当场没红，才发现。

顺手把全部十个用剥离 helper 的扫描类断言按同一条判据过了一遍：

| 剥离口径 | 文件 |
| --- | --- |
| 只剥注释（判字符串内容**安全**） | `MentionsFilterDefaultOn` / `TodoMarkdownRendering` / `TodoBlockedOnHuman` / `DecisionKindHasNoProducer` / `CockpitOpenCloseCost` / `AskIntoTodo` / `PermissionIntoTodo` / `TodoDroppedAndAttention` / `ViewWiring` |
| 剥注释**并剥字符串** | `CaptainTodoSweepTests`（就是踩坑那个，已加 `inCodeOnly` 参数） |

判据是文件里有没有 `inString` 那段逐字符状态机 —— 有它就会把字符串内容一起吃掉。
`DecisionKindHasNoProducerTests` 值得单独说一句：它筛的是 `kind: "decision"`
这种**字符串字面量**，而它的 helper 只剥注释，所以成立。

**这一条是读数不是待办**：九个「安全」是按剥离口径判的，**不等于它们各自都变异证过**。
要证只能一条条来（每条造一次它要禁的那个写法）。**别把这张表读成「九条都是真的会红」。**
