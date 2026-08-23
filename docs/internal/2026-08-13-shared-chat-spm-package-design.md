## 已搁置（2026-08-23，机长决定）

**这份文档不是活的计划，也不是一件排着队待做的事。这条线已由机长决定搁置 —— 不开工，也不做「先重扫一遍漂移表」那一步（重扫本身就是投入，且扫完的结论大概率仍是不开工）。**

搁置的三条理由，都是 2026-08-23 在两个仓库上核实的：

1. **#575 已经在两边各自修好了，抽包最硬的那个卖点没了。** 2026-08-19～08-20 PendingBot 在候选面上落了 7 笔，其中 `9aa9cc33 mac: 输入框换 NSTextView — 修中文输入法压字 + 点击后焦点掉回` 正是 #575，另有 `c1bbee91`（附件缩略图降采样）、`052abd8d`（围栏代码块渲染）。这三条正是本文 §0 用来论证「抽包完成的那一刻两端各自缺的修复自动补齐」的例子 —— PendingBot 自己各修了一遍。收益因此从「两个 app 同时修好」降级成「省重复行数」。
2. **漂移表整个失效，必须重扫。** 上面那 7 笔全部落在候选面上，本文 §1 / §5.1 的逐文件漂移结论、以及「候选面零改动、结论原样成立」那句抬头，都不再成立。而且两边现在各有**一份独立实现的同一个修复**，三方合并比 08-13 时更难，成本是涨的不是降的。
3. **monorepo 假设作废。** 本文通篇假设 `apps/pendingcrew` / `apps/pendingbot` / `apps/shared/PendingChatKit` 并排，而 PendingCrew 已是独立仓库、根本没有 `apps/` 目录（这条 2026-08-20 的恢复说明里已经记过）。落地形态得重定成真正独立发布的 SPM 包 + 两个仓库各自声明依赖。

**重启这条线的前提**：先出一份**新的漂移扫描**（在当时的两个仓库 HEAD 上重做 §1 / §5.1 的对账）＋一份**新的收益估算**。本文里的任何数字都不能直接拿来支持重启。

**什么时候值得去做那件事**（触发器是疼点，不是日期）：哪天真出现「同一个 bug 要在两边各修一遍」的具体场景，拿那次疼点当触发器重估。在那之前不要开工，也不要重扫。

一条顺带澄清，免得后人误会：这活曾被「等人类对 PendingCrew 后端归属拍板」按住过，**那个理由按不住它** —— 候选面 18 个文件是纯 UI（主题 / 气泡 / 输入框 / Markdown），不碰 `Auth/`、不碰 edge、不碰 DB。2026-08-23 iOS 整条线搁置也不影响它。真正卡它的只有上面那个投入产出。

以下内容（含 2026-08-20 的恢复说明和原文）原样保留，供将来重估时参考。

---

> ## 恢复说明（2026-08-20 补，非原文）
>
> **这一整块是恢复时补的元信息，`---` 分隔线以下是原文，一字未改。**
>
> | | |
> |---|---|
> | 原路径 | `docs/superpowers/specs/2026-08-13-shared-chat-spm-package-design.md`（旧 monorepo 内） |
> | 原提交 | `7026640a`（纯 docs，直接落 main，未 push） |
> | 为什么现在解析不出来 | **不是文件丢失。** 本仓库的 git 历史只回溯到 2026-08-15 —— 它是从 monorepo 快照 `61d876a7` 压平重建的（首个提交 `a61e7a5`，见 `docs/architecture.md` §1.6）。重建前的 sha 一律解析不出来是设计使然。丢的只有这份文档本身：它当时提交在 `docs/superpowers/` 下，而该目录未随快照进入本仓库。 |
> | 正文从哪恢复 | Claude Code 的 session transcript（JSONL）中的 `Write` 工具调用**原始入参**，不是群聊转述。出自 `~/.claude/projects/-Users-hey-Untitled-Pendingname-PendingCrew/f3fefb24-7971-4508-96d5-f439d05840aa.jsonl`（2026-08-12T16:52Z，8927 字符 / 209 行）。**该 session 对这个文件只写过一次、此后没有任何 `Edit`**，所以这一次 `Write` 就是最终版，无需重放。 |
> | 完整性 | **完整。** 没有任何一节是补写或转述的。 |
> | 恢复日期 | 2026-08-20 |
> | 恢复后的落点 | `docs/internal/`（本仓库无 `docs/superpowers/` 那套目录，不为此新建） |
>
> ### 附录 A 是什么
>
> 同一个 session 更早（2026-08-12T10:29Z）在 scratchpad 里写过一份对账草稿 `spm-chat-package-plan.md`（7406 字符 / 151 行），也一并从 transcript 里捞了出来。逐节比对后：**这份 spec 是那份草稿的超集**（逐文件漂移表、撞面分支表、缝的清单、分批表、验证门、明确不做，全部已在正文中，且措辞更完整），**只有一节草稿有而 spec 没有** —— 冷/热文件的划分依据。那一节原样附在文末附录 A，其余不重复收录。
>
> ### 这份文档现在的状态
>
> 原文状态行（`---` 下方第 4 行）写的是「**设计已获机长认可（2026-08-12 机长），尚未动工——挪文件需机长在机器还原完成后另行放行**」。**恢复不改变这一行的任何一个字，也不构成放行。**（该状态行已于 2026-08-23 改为「已搁置」，理由见文首「已搁置」一节；原措辞在下方括注里原样保留。） 截至 2026-08-20，`apps/shared/PendingChatKit/` 不存在、一个文件都没挪过。
>
> ### 一处比 sha 更要紧的过期：仓库布局已经变了
>
> 原文通篇假设一个 monorepo（`apps/pendingcrew` / `apps/pendingbot` / 新建 `apps/shared/PendingChatKit` 并排）。**那个布局今天不存在了** —— 本仓库根本没有 `apps/` 目录，PendingCrew 的源码直接在仓库根的 `Sources/` 下，PendingBot 是同级的另一个独立仓库（`/Users/hey/Untitled/Pendingname/PendingBot`）。
>
> 这不推翻文档的结论（漂移是双向的、每个文件要三方合并、§2 的根因诊断、三笔债的落点、验证门），但**推翻它的落地形态**：跨仓库共享不能再靠 `apps/shared/` 一个路径，得是真正独立发布的 SPM 包 + 两个仓库各自声明依赖。动工前这一节必须重定。
>
> 另需注意：原文中的**基线 HEAD `b92f63f2`、对账基线 `28398bf4`、以及 §5.1 列出的那批 commit sha，都是重建前的旧 sha，在本仓库同样解析不出来**（原因同上）。它们记录的是当时的对账事实，动工前应按当时的判断方法在**当前** HEAD 上重新扫一次 —— 这正是原文 §5.1 已经要求过的动作。

---
# 共享聊天 SPM 包设计 —— 把 PendingCrew 的 vendored 聊天件和 PendingBot 的原件收口成一份

**日期**：2026-08-13
**状态**：**已搁置（2026-08-23，机长决定）**，作废理由与重启前提见文首「已搁置」一节。（此行原措辞：「设计已获机长认可（2026-08-12 机长），**尚未动工**——挪文件需机长在机器还原完成后另行放行」）
**基线 HEAD**：`b92f63f2`。对账实测在 `28398bf4` 做，已核 `28398bf4..b92f63f2` 对候选面（`apps/pendingcrew/Sources/Chat`、`apps/pendingbot/Sources/{Components,Features/Message,Storage/AppearanceMode.swift}`）**零改动**，故全部结论在当前 HEAD 上原样成立。
**范围**：`apps/pendingcrew` + `apps/pendingbot` + 新建 `apps/shared/PendingChatKit`。**不含** edge、不含 DB。
**关联**：task #575（PendingBot Mac 输入框四 bug）、`docs/tech-debt.md` 黄字降级规则双份那条、`docs/qa/2026-07-26-pendingcrew-device-qa.md`（#443 活验批次）

---

## 0. 一句话结论

**这不是「删重复」，是「两个 app 同时修好」。**

因为漂移是**双向**的：PendingBot 有 PendingCrew 没有的（Mac 排版、发送者标签、猜/换模型菜单），PendingCrew 也有 PendingBot 没有的（**Mac 输入框的 NSTextView 实现、代码块渲染修复、⌘V 粘图、附件降采样**）。所以既不能「删掉 crew 的副本指回 bot」，也不能反过来 —— **每个漂开的文件都要三方合并**。

这句话直接决定两件事：① 工作量在合并而不在搬运；② 抽包完成的那一刻，两端各自缺的修复自动补齐，这是它真正的价值。

---

## 1. 对账（实测，非转述）

### 1.1 vendored 清单

`apps/pendingcrew/Sources/Chat/Vendored/`，**18 个文件 3802 行**。每个文件头都带 `// VENDORED from PendingBot <路径> @ <commit>`，所以漂移量可精确计算而非估计。

| 文件 | vendor 基线 | bot 侧此后漂移 | crew 侧 SHIM 处数 | 近 14 天热度 |
|---|---|---|---|---|
| Platform.swift | c63d3989 | — | 0 | 冷 |
| PlatformImage.swift | c63d3989 | — | 0 | 冷 |
| PlatformModifiers.swift | c63d3989 | 1 commit / +16 | 0 | 冷 |
| ColorHex.swift | c63d3989 | — | 0 | 冷 |
| ColorHash.swift | c63d3989 | — | 0 | 冷 |
| Haptics.swift | c63d3989 | — | 0 | 冷 |
| BlinkingCursor.swift | c63d3989 | — | 0 | 冷 |
| BotAvatar.swift | c63d3989 | — | 0 | 冷 |
| Theme.swift | c63d3989 | **5 commits / +54** | 0 | 冷 |
| AppearanceMode.swift | 43c8ea2e | — | 1 | 冷 |
| CodeRunnerSheet.swift | 43c8ea2e | — | 1 | 冷 |
| MathMarkup.swift | 43c8ea2e | — | 1 | 热(1) |
| MathRendering.swift | 43c8ea2e | — | 1 | 热(1) |
| MarkdownText.swift | 43c8ea2e | — | 6 | 热(2) |
| ChatActionSheet.swift | 43c8ea2e | 1 commit / +27 | 1 | 热(1) |
| BubbleView.swift | 43c8ea2e | **3 commits / +52 −4** | 11 | 热(2) |
| ComposerView.swift | 43c8ea2e | 1 commit / +7 −1 | 18 | 热(1) |
| AppThemeAlias.swift | （crew 独有） | — | — | 热(1) |

同目录另外两块**不进包**：`Shims/`（302 行）、`Adapter/`（1985 行）—— 宿主粘合层，本来就该留在 app 侧。

### 1.2 漂移方向

**PendingBot 领先（crew 缺）**

- `Theme` +54 行，**纯加法**：Apple/Google 登录按钮的深色 token、`scriptTitle`（中英混排按串内是否含 CJK 选字体 design）、`conversationGutter`。crew 侧的 Mac 排版整整落后一轮。
- `BubbleView` +52/−4：relay 发送者标签、右键/长按头像 @ 该发送者、Mac 顶部留白。
- `ChatActionSheet` +27 / `ComposerView` +7：`+` 面板的「猜模型 / 换模型」——PendingBot 独有功能，抽包后必须是可选注入而不是包里的固定项。
- `PlatformModifiers` +16：Mac picker sheet 最小尺寸。

**PendingCrew 领先（bot 缺）**

- `ComposerView`：`2f693319` 把 Mac 输入框换成 NSTextView（+179/−31），中文候选态 placeholder 立即消失；另有 ⌘V 粘图拦截、焦点挂 responder 生命周期。
- `MarkdownText` / `MathRendering` / `MathMarkup`：三反引号代码块渲染修复（`5271e097`）、MathMarkup 拆成纯 Foundation 文件。
- `BubbleView`：本地 `file://` 附件交系统打开、图按格子尺寸降采样解码、逐条 textSelection（#443 性能）。

### 1.3 包外还有一份重复：黄字根 crew 降级规则

- PendingCrew：`Sources/Views/CrewRootBadge.swift`（183 行，含可测的 `CrewTitleRootBadgeLayout` 纯 Layout）+ 单测。
- PendingBot：内嵌在 `Sources/Features/Message/MessageTabView.swift` 的 `PendingBotCrewTitleRootBadgeLayout`，**逐字复制、零单测**——`apps/pendingbot` 根本没有 test target（`project.yml` 里没有任何 test 声明）。
- 口径已经改过一轮（省略号 → `+N`）。再改一次，改一边不会红、没人提醒，手机会话列表会**静默漂成另一套显示**。

---

## 2. 根因诊断：为什么会漂开

**当前代码把两类差异混在了同一种表达里。**

- **平台差异**（iOS vs macOS 的 API 不同）→ 用 `#if os(...)`，这是对的。
- **宿主差异**（PendingBot 用 `APIClient`、PendingCrew 用 `CrewAttachmentDownload`；crew 的气泡要叠机长星标、bot 不要）→ 现在也被就地改写成注释标记 `// PENDINGCREW SHIM:`，**共 18 处**。

标记只能告诉后人「这里被改过」，**不能阻止下一次改**。一份被就地改写 18 处的副本，本质上已经是第二份实现 —— 它必然继续漂，只是漂得慢一点。

**所以本设计的核心规矩是：平台差异用 `#if`，宿主差异用注入。** 这不是风格偏好，是上面这段漂移史的根因诊断。抽包的动作不是「把 shim 搬进包」，而是把每一处 shim 的**原因**变成一个显式注入点。

---

## 3. 包结构

### 3.1 位置与命名

```
apps/shared/PendingChatKit/
  Package.swift
  Sources/ChatKitCore/     # 零外部依赖
  Sources/ChatKitUI/       # 依赖 ChatKitCore + MarkdownUI + SwiftMath
  Tests/ChatKitTests/
```

放 `apps/shared/` 的理由：那里已经是两个 app 共享 Swift 代码的地方（`AppUpdate` 走 `- path: ../shared/AppUpdate` 被两端直接拉进 target）。这次是把「共享靠路径包含」升级成「共享靠真 SPM 包」，两端改用 `packages:` + `dependencies:`。

顺带清掉 `apps/shared/PendingBotShared/` —— 只剩 `.build/` 和 `.swiftpm/` 的空壳、**未跟踪**，是早年一次没做完的 SPM 尝试，留着会误导下一个人以为已经有共享包了。

### 3.2 分层

**`ChatKitCore`**（无外部依赖，纯 SwiftUI / Foundation）
`Platform`、`PlatformImage`、`PlatformModifiers`、`ColorHex`、`ColorHash`、`Haptics`、`BlinkingCursor`、`AppearanceMode`、`MathMarkup`、`Theme`（含 `AppThemeAlias` 的消歧机制）、**`RootBadgeLayout`**（由两端各自的副本合成）

**`ChatKitUI`**（依赖 Core + MarkdownUI + SwiftMath）
`MarkdownText`、`MathRendering`、`CodeRunnerSheet`、`BotAvatar`、`BubbleView`、`ChatActionSheet`、`ComposerView`

**`ChatKitTests`**
接收 PendingCrew 现有的黄字降级单测，以及 `CrewChatAdapterTests` 中属于包的那部分。
→ **副作用值得写明：PendingBot 由此第一次拥有覆盖这批代码的 Swift 单测**（它自己没有 test target，靠依赖包获得保护）。

### 3.3 缝：18 处 SHIM 变成显式注入点

| 缝 | 现在被 shim 掉的东西 | 包里的形态 |
|---|---|---|
| 附件取字节 / 打开 | crew 用 `CrewAttachmentDownload` + `NSWorkspace` 开 `file://`；bot 用 `APIClient().download` | `protocol ChatAttachmentLoader`，宿主注入实现 |
| 远端图 / 头像 | crew 的 `Shims/ServerImage`+`UserAvatar`；bot 自己那份（缓存策略不同） | environment 注入的 view factory；**图片缓存策略不进包** |
| 气泡头像徽章 | crew 的 `CrewAvatarBadges`（机长星标 / 状态点）；bot 没有 | 可选 `ViewBuilder`，默认空 |
| 发送者行 | crew 的 `GroupBubbleSender`；bot 的 relay 标签 | 同上，可选 `ViewBuilder` |
| composer 附加件 | placeholder（随发送态切换）、Todo 钮、`onPasteAttachments` 粘贴拦截、`+` 面板条目（bot 的猜/换模型） | `ComposerExtras` 参数包，**不用 `#if` 区分宿主** |
| 逐条可选中 | crew 因 #443 性能改成开关，bot 常开 | `chatTextSelectable` 环境值，宿主给默认 |
| 外观模式存储键 | crew 改了 UserDefaults key 免撞 | `AppearanceMode(storageKey:)` 初始化参数 |

---

## 4. 三笔债的收口（抽包是唯一收口点，不留「以后再说」）

### 4.1 #575 —— PendingBot Mac 输入框 IME bug

**这条的成本量级变了，必须写清楚。**

实测（`b92f63f2`）：

| | 行数 | `NSTextView` 出现次数 |
|---|---|---|
| `apps/pendingcrew/Sources/Chat/Vendored/ComposerView.swift` | 917 | **18** |
| `apps/pendingbot/Sources/Features/Message/ComposerView.swift` | 608 | **0** |

也就是说 **#575 的成品修复已经躺在 PendingCrew 这边**，PendingBot 的 Mac 分支还是纯 SwiftUI TextField。

**落点**：第 5 批。ComposerView 合成时 Mac 分支直接采用 crew 的 NSTextView 实现，PendingBot 换成依赖包即自动获得。
**这笔债因此从「要实现」降级成「换依赖即得」** —— 派活和排期都应按后者算。
**活验**（命令行造不出，归 #443）：Mac 上的 PendingBot 用中文输入法打字，候选态出现的一刻 placeholder 必须立即消失。

### 4.2 黄字降级规则两份

**落点**：第 6 批。`RootBadgeLayout` 进 `ChatKitCore`；删掉 `MessageTabView.swift` 里的 `PendingBotCrewTitleRootBadgeLayout`；PendingCrew 那 183 行单测搬进 `ChatKitTests`。

**明确不做**：不在 PendingBot 里补一份平行单测。那只是把两份变成两份带测的，照样会漂。

### 4.3 PendingBot 没有任何 Swift test target

**落点**：不新建 app 级 test target（那是另一件事，本次不夹带）。但包自带 test target，抽走的这 4000+ 行从此有测试保护。
tech-debt 上那条改成「已随抽包收口，剩余未覆盖面：app 自身逻辑」——**不删原文，就地加带日期的批注**。

---

## 5. 风险面与撞车

### 5.1 vendored 目录近期提交（全在基线内）

`077a7463` 叉按钮底影 / `5271e097` 代码块不渲染 / `a1fe7c3f` MathMarkup 拆文件 / `804213cc` 按需窗口 + 逐条 textSelection / `4f6fbb44` 附件图降采样 / `a20a96d3` 拖文件发送 + 死钮 / `7ff1d1f3` ⌘V 粘图 / `8163f8f2` Mac 焦点。
更早但关键的 `2f693319`（#492 NSTextView）也在，已用 `git merge-base --is-ancestor` 核过。

**基线取 main 覆盖以上全部。** 动工前仍应再扫一次 `git log --oneline -- apps/pendingcrew/Sources/Chat/Vendored/`——漏掉任何一笔，等于把修好的 bug 随共享包重新发一遍。

### 5.2 未合分支中碰到候选面的，只有两条

| 分支 | 碰什么 | 判断 |
|---|---|---|
| `pendingcrew/session-0f6db2`（#574 系统消息身份） | `Chat/Adapter/` 三件 + `Stores/` + `Mac/` | **不撞** —— Adapter 不进包，`Vendored/` 一个字节没动，与该线的承诺一致 |
| `pendingcrew/session-8fa389`（黄字那条线） | `apps/pendingbot/.../MessageTabView.swift`（+45/−4） | **撞第 6 批** —— 正是要删副本的那个文件；动那批之前先与该线对齐 |

**近 14 天的热区全在 `Adapter/`（8 个提交），而 Adapter 整块不动。** 这是本次抽包与其他部门撞面小的根本原因，也是可以先行的依据。

---

## 6. 分批落地

每批可单独编译、单独合 main。

| 批 | 内容 | 冲突面 |
|---|---|---|
| 0 | 建包骨架 + 空 target + 两端 `project.yml` 加依赖（**不搬任何文件**） | 只碰 project.yml / pbxproj —— 先证明 SPM 接线通再谈搬运 |
| 1 | 8 个冷叶子件（Platform / PlatformImage / PlatformModifiers / ColorHex / ColorHash / Haptics / BlinkingCursor / BotAvatar） | 零：两边零漂移或纯加法 |
| 2 | Theme（union，bot 的 +54 全收）+ AppearanceMode（key 变参数）+ AppThemeAlias 机制 | 低：冷且纯加法 |
| 3 | MarkdownText / MathRendering / MathMarkup / CodeRunnerSheet（crew 的代码块修复带给 bot） | 中：热，但漂移方向单一 |
| 4 | BubbleView + ChatActionSheet（缝最多：附件、徽章、发送者行、选中态） | 高 |
| 5 | **ComposerView**（最大最险；crew 领先 179 行 Mac 实现；#575 在这批收口） | 最高 |
| 6 | RootBadgeLayout 合成 + 删 PendingBot 副本 + 搬单测 | 与 `pendingcrew/session-8fa389` 撞，先对齐 |
| 7 | 删 `Chat/Vendored/` 整目录 + 两端 project.yml 收口 + xcodegen + 三端全验 | 收尾 |

### 6.1 每批的验证门（一条都不省）

- **PendingCrew**：`-destination 'platform=macOS' test` **加上** `-destination 'generic/platform=iOS Simulator' build`。三端共用一套源码，只编 Mac 会让漏 `#if os(macOS)` 的 API 静默把 iOS/iPad 打红而 Mac 侧毫无异常。
- **PendingBot**：iOS Simulator build **加上** macOS build。
- **两个工程都无条件重跑 `xcodegen` 并核 diff**，不管有没有报冲突 —— pbxproj 文本合并零冲突也会把生成值倒回去（`MARKETING_VERSION` 已被这么坑过一次，见 `docs/tech-debt.md` 2026-08-11 条目与 `CLAUDE.md` 合并小节）。
- `CrewChatOpenCostTests` 需先跑 `scripts/make-chat-fixtures.sh`，否则 14 项硬红且与本次改动无关。

---

## 7. 明确不做

- **不碰 `apps/pendingcrew/Sources/Auth/`** —— #574 那条线在飞，双方对等豁免。
- **不碰 `Chat/Adapter/`、`Chat/Shims/`** —— 宿主粘合层，本来就该留在 app 侧。
- **不顺手改任何业务行为。** 本次全部是搬运 + 三方合并 + 加注入点。合完两端的行为差异**只允许**来自第 4 节那三笔债的收口，其余逐路等价。

---

## 附录 A：来自同 session scratchpad 草稿 `spm-chat-package-plan.md` 的补充（原文，2026-08-12T10:29Z）

> 收录理由与范围见文首恢复说明。以下为草稿 §2.3 原文，是本 spec 正文中未出现的唯一一节。

### 2.3 冷/热分批依据
冷（近 14 天无人碰，11 个）：Platform / PlatformImage / PlatformModifiers / ColorHex / ColorHash / Haptics / BlinkingCursor / BotAvatar / Theme / AppearanceMode / CodeRunnerSheet
热（7 个）：MarkdownText / MathRendering / MathMarkup / BubbleView / ChatActionSheet / ComposerView / AppThemeAlias
