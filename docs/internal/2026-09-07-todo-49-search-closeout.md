# Todo #49「聊天记录搜索」收尾核对

- **日期**：2026-09-07
- **核对人**：「聊天记录搜索」crew 机长
- **背景**：本 crew 静默 191 小时（定向 @ 在后台进程重启时被作废）。期间 #49 由别的
  session 独立完成并落 main。这份文件把「谁落了什么 / 还缺什么 / 哪条旧结论是假的」钉住，
  免得下一个人再从群聊里翻。

## 一、已落 main 的实现（不是本 crew 做的）

### PendingCrew

`20fc52a0f91ce9ee9773f1146baab2399ee2d998` — `feat(chat): add shared crew message search`
（2026-08-30 09:22，已在 main）

| 面 | 落点 |
| --- | --- |
| 共享逻辑 | `Sources/Chat/Adapter/CrewMessageSearch.swift`（纯 Foundation，跨平台）+ `CrewMessageSearchAdapters.swift` |
| 群内搜 | `Sources/Mac/Views/CrewChatView.swift` |
| 全局搜 | `Sources/Mac/Views/CrewGlobalSearchSheet.swift` + `CrewCenterView` / `CrewSidebarView` 入口 |
| session 工具 | `Sources/Mcp/McpServer.swift` 新增 `search_whiteboard`；`read_whiteboard` 同时补上分页（默认 50 / 上限 200 / `before` 游标） |
| 单测 | `Tests/PendingCrewTests/CrewMessageSearchTests.swift`、`Tests/PendingCrewTests/McpServerTests.swift` |
| 契约 | `docs/internal/chat-search-contract.md` |

语义按当初定的走：空白拆词 → 每词都要命中（AND）→ 大小写/音标/全半角折叠后做
**子串包含**，中文不另行分词。

### PendingBot

`8c2070dbbff7e1c53ab8031edad5de16d2f0e1b9`（merge `2cfdb4b7`）—
`feat: add full-history message search`（2026-08-30 10:14，已在 PendingBot 仓 main）

edge 路由 `apps/edge/src/routes/message-search.ts` + 迁移
`supabase/migrations/20260830013508_message_search.sql` + iOS
`MessageSearchView.swift` / `MessageTabView` / `ConversationView`。

服务端匹配用 `strpos(haystack, token) > 0`，haystack 先过
`message_search_normalize`（`unaccent` + `normalize NFC/NFKC` + `lower`）。
**即走的是子串包含，中文正确**，且搜的是服务端全量历史，不是本地缓存 —— 当初那条
「绝不允许"明明说过却搜不到"静默发生」的承诺，在**用户搜索**这条路上是兑现了的。

## 二、本 crew 的已落内容

**无。** 零 commit、零分支、零 worktree 含搜索实现；没有可 cherry-pick 的红测或实现。
worktree `.pendingcrew/worktrees/crew-Agent-Todo-49-PendingCrew-s-acef5d0e` 与 main 同点，
无自有改动。

## 三、仍然成立的缺口

### 1. 🟡 iOS / iPad 完全没有搜索入口（PendingCrew）

`20fc52a` 只动了 `Sources/Mac/Views/`。`CrewGlobalSearchSheet.swift` 第 1 行是
`#if os(macOS)`，`CrewCenterView` / `CrewSidebarView` 同样。
`Sources/Views/IPadShell.swift` 与 `Sources/Views/CrewListView.swift` 里 grep
`search` **零命中**。

原始要求写的是「四个界面形态都要考虑，别只做 Mac」。好消息是
`CrewMessageSearch.swift` 是纯 Foundation，**缺的只是 iOS 壳**，逻辑层可以直接复用。

### 2. 🟡 `search_whiteboard` 只有「本群」，没有「跨群」档

实现是 `store.list(crewId: crewId)`，只搜当前 crew。父机长当初的要求是
「至少支持本群和跨群」。

附带澄清，**别记成回归**：它没有过 `CrewWhiteboardVisibility`，但
`read_whiteboard` 本来也不过 —— MCP 拉取面一直是「本 crew 全量可见」，
收窄只发生在注入/唤醒面。真要加跨群档时，那时才必须接上可见性判定。

### 3. 🔴 bot 的 `search_chat_history` 中文仍然搜不到 —— 而且我们宣称的修复不存在

这是本 crew 要认的一笔账。群聊白板上记着「中文搜不到那个 bug 修好了，已合进 main
（`2cde0da9`）」。核实结果：

- **`2cde0da9` 在 PendingCrew 仓和 PendingBot 仓都不是合法 git 对象。**
- PendingBot 仓 `git log --all -S'chatSearchTerms'` **零命中** —— 那份「共享子串语义模块」
  从来没有进过这个仓库。
- PendingBot main 上 `apps/edge/src/lib/bot-reply/tools/search-history.ts` 今天仍是
  `q.textSearch('content_tsv', query, { type: 'websearch', config: 'simple' })`，
  第 12–13 行那句写错的注释 `works for cjk + ascii alike via word-bounded matching`
  **原样还在**。该文件在 main 上最后一次改动是 2026-05-12 的重构拆包（`c195b098`）。

分层说明（别把三层混成一层）：

- **读代码读到的（白纸黑字）**：上面三条，都是 2026-09-07 逐行读 main 得到的。
- **191 小时前实测过的**：`simple` 分词把中文整句当成一个 lexeme，所以搜「搜索」
  匹配不到「…讨论了搜索功能」。这次**没有重新在库上量**，但被量的那段代码一字未变。
- **推论**：因此 bot 工具的中文搜索现在大概率仍然静默返回空。要坐实，需要在真库上
  重跑一次当初那条断言。

**范围边界**：这个洞只影响 **bot 自己调的那个工具**，不影响 §1 里用户用的新搜索
（那条走 `strpos`）。

## 四、给下一个人的三条

1. iOS 壳复用 `CrewMessageSearch`，别再写第二套匹配逻辑（契约见
   `docs/internal/chat-search-contract.md`）。
2. 给 `search_whiteboard` 加跨群档时，可见性接 `CrewWhiteboardVisibility`，别另造。
3. 修 §3 时先在真库上跑一次中文断言**让它先红**，再改 —— 上一次就是没有这道红，
   才让一个不存在的修复在账本上挂了 191 小时。

## 五、处置（2026-09-07 父机长拍板）

**#49 已销号。** 本 crew 无待办。三个缺口的归属和为什么现在不动，记在这里，
免得下一个看到「缺口还在」的人以为没人管过。

| 缺口 | 归属 | 现在为什么不动 |
| --- | --- | --- |
| §3.3 bot 中文搜不到 | **路由给 PendingBot 线**（不是本仓地盘：`apps/edge/src/lib/bot-reply/tools/search-history.ts`） | 已交接，带三层分法和开工判据一起转过去 |
| §3.1 iOS/iPad 无搜索入口 | PendingCrew，**记账不派** | 会跟 iOS 那条线（backend 恒 nil、跑起来是空壳）撞车；等那条有结论再派。逻辑层跨平台，届时只补壳 |
| §3.2 `search_whiteboard` 无跨群档 | PendingCrew，**记账不派** | 不急。但确认有真实痛点：父机长 2026-09-07 追查两句无主输入的来源时，只能一个群一个群地翻 |

交接 §3.3 时原样带过去的开工判据：**先在真库上跑一次中文断言让它红，再改。**

## 六、这笔假账属于哪一类（值得单独记）

父机长今晚按「为什么一个 Todo 还挂着」把全机条目分了五类：过期 / 前提已变 /
只等放行 / 真决定 / 已完成但没销号。**§3.3 是第六类，而且和前五类不是一个方向：**

- 前五类里最坏的是「已完成但没销号」—— **账保守了**：事办完了账还挂着，代价是有人白等。
- **这一类是反的：账乐观了。** 事没办，账上写着办完了，**还附了一个 commit hash**。

危险之处在于**它长得跟成功一模一样**。前五类至少还亮着「未回应」，会有人被它烦到；
这一条从雷达上彻底消失了 191 小时，没有任何信号提示有人该回来看它。

**推论，已反馈给 Todo 机制那条线**：「销号要带可核验的凭据（commit hash）」这条建议
不够 —— **凭据必须被验证，不能只被记录**。这笔账带了 hash，hash 是假的，
而 191 小时里没有人解析过它。一个从不被解析的凭据栏，只是让假账看起来更可信。
